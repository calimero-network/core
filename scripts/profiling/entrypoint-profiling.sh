#!/bin/bash
# Profiling-enabled entrypoint for merod

set -e

ENABLE_PROFILING="${ENABLE_PROFILING:-true}"
ENABLE_JEMALLOC="${ENABLE_JEMALLOC:-true}"
ENABLE_PERF="${ENABLE_PERF:-true}"
ENABLE_HEAPTRACK="${ENABLE_HEAPTRACK:-false}"
ENABLE_WASMER_PROFILING="${ENABLE_WASMER_PROFILING:-true}"
PROFILING_OUTPUT_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
PERF_SAMPLE_FREQ="${PERF_SAMPLE_FREQ:-99}"

# Periodic preserve+render cadence. The default of 60s means a regression
# that kills the container mid-run still leaves a flamegraph at most ~60s
# stale on the bind mount — instead of leaving an empty `reports/` dir (or
# no dir at all) when the final shutdown trap is cut short by Docker's
# stop-timeout. Set `PRESERVE_INTERVAL_SECONDS=0` to disable.
PRESERVE_INTERVAL_SECONDS="${PRESERVE_INTERVAL_SECONDS:-60}"

MAIN_PID=""
EXIT_CODE=0
PRESERVE_LOOP_PID=""

mkdir -p "$PROFILING_OUTPUT_DIR"
mkdir -p "${PROFILING_REPORTS_DIR:-/profiling/reports}"

# Best-effort: eagerly create the bind-mount reports dir at startup and open
# a per-node status log there. `preserve_to_host_mount` does this too on
# every preserve, but doing it here means a container killed *before*
# perf/merod even start still leaves a visible `<bind>/profiling-dump/
# profiling-status.log` so harvest-side diagnosis isn't blind. `:-` so an
# unset `CALIMERO_HOME` (running outside merobox) is a no-op.
EAGER_BIND_DEST="${CALIMERO_HOME:-}/profiling-dump"
PROFILING_STATUS_LOG=""
if [ -n "${CALIMERO_HOME:-}" ] && [ -d "$CALIMERO_HOME" ]; then
    mkdir -p "$EAGER_BIND_DEST/reports" 2>/dev/null || true
    PROFILING_STATUS_LOG="$EAGER_BIND_DEST/profiling-status.log"
    : > "$PROFILING_STATUS_LOG" 2>/dev/null || PROFILING_STATUS_LOG=""
fi

# Append a timestamped diagnostic line to the persistent status log (if it
# could be opened) AND echo to stdout. Use this for the few key decisions
# the harvester needs to be able to see post-mortem (perf-record start,
# perf-sanity fail, flamegraph render success/fail) — *not* every
# `[Profiling] ...` echo (those still go to stdout and would bloat the log).
profiling_status() {
    local line
    line="$(date -u +%Y-%m-%dT%H:%M:%SZ) ${NODE_NAME:-merod} $*"
    echo "[Profiling] $*"
    [ -n "$PROFILING_STATUS_LOG" ] && echo "$line" >> "$PROFILING_STATUS_LOG" 2>/dev/null
}

install_kernel_tools() {
    local kernel_version=$(uname -r)
    echo "[Profiling] Detected kernel: $kernel_version"

    # Echo perf's own stderr on sanity-check failure so we can tell missing
    # binary / ABI mismatch / EPERM (missing CAP_PERFMON) apart.
    perf_sanity_check() {
        local err
        err=$(perf record -o /dev/null -- true 2>&1)
        if [ $? -eq 0 ]; then
            return 0
        fi
        echo "[Profiling] perf sanity check failed. First 5 lines of stderr:"
        echo "$err" | head -5 | sed 's/^/[Profiling]   /'
        return 1
    }

    if perf_sanity_check; then
        echo "[Profiling] perf is compatible with current kernel"
        return 0
    fi

    echo "[Profiling] Installing kernel tools..."
    apt-get update -qq 2>/dev/null || true

    if apt-get install -y -qq "linux-tools-${kernel_version}" 2>/dev/null; then
        if perf_sanity_check; then
            echo "[Profiling] perf is now working (linux-tools-${kernel_version})"
            return 0
        fi
    fi

    # Fallback: linux-tools-generic (pre-installed in the profiling image;
    # try apt again for non-profiling deployments). The Ubuntu /usr/bin/perf
    # wrapper requires /usr/lib/linux-tools/$(uname -r)/perf, so symlink the
    # generic binary into place.
    echo "[Profiling] Trying linux-tools-generic fallback..."
    apt-get install -y -qq linux-tools-generic 2>/dev/null || true
    local generic_perf=""
    for candidate in /usr/lib/linux-tools/*/perf; do
        [ -f "$candidate" ] || continue
        # basename compare avoids regex metacharacter traps in kernel_version.
        [ "$(basename "$(dirname "$candidate")")" = "$kernel_version" ] && continue
        generic_perf="$candidate"
        break
    done
    if [ -n "$generic_perf" ]; then
        local target_dir="/usr/lib/linux-tools/${kernel_version}"
        if ! mkdir -p "$target_dir" 2>/dev/null; then
            echo "[Profiling] WARNING: could not create $target_dir"
        elif ! ln -sf "$generic_perf" "$target_dir/perf" 2>/dev/null; then
            echo "[Profiling] WARNING: could not symlink $target_dir/perf -> $generic_perf"
        elif perf_sanity_check; then
            echo "[Profiling] perf working (linux-tools-generic via $generic_perf)"
            return 0
        fi
    fi

    echo "[Profiling] WARNING: CPU profiling unavailable."
    echo "[Profiling]   If stderr above shows 'Operation not permitted', container is missing CAP_PERFMON."
    return 1
}

start_profiling() {
    local pid=$1
    local node_name="${NODE_NAME:-merod}"

    profiling_status "start_profiling: pid=$pid node=$node_name"

    if [ "$ENABLE_PERF" != "true" ]; then
        profiling_status "perf disabled via ENABLE_PERF=$ENABLE_PERF, skipping CPU profiling"
        return
    fi

    # Capture stderr of the perf-sanity check so a failure here ("Operation
    # not permitted" → missing CAP_PERFMON, "perf_event_open failed" →
    # paranoid=4, missing binary, etc.) is *visible* in the bind-mount
    # status log — not just on stdout, which evaporates with the container.
    # Without this, "only N of 4 nodes had perf data" is impossible to
    # diagnose from the harvested artifact alone.
    local sanity_err
    sanity_err=$(perf record -o /dev/null -- true 2>&1)
    if [ $? -ne 0 ]; then
        profiling_status "perf-sanity FAILED on this node, skipping CPU profiling"
        # Log the first 3 lines of perf's own error so we can tell EPERM
        # (missing CAP_PERFMON) apart from missing binary / ABI mismatch.
        printf '%s\n' "$sanity_err" | head -3 | while IFS= read -r l; do
            profiling_status "  perf-sanity stderr: $l"
        done
        return
    fi

    if ! kill -0 "$pid" 2>/dev/null; then
        profiling_status "target pid=$pid is not running, cannot start perf"
        return
    fi
    
    local perf_output="$PROFILING_OUTPUT_DIR/perf-${node_name}.data"
    local perf_log="$PROFILING_OUTPUT_DIR/perf-${node_name}.log"
    
    echo "[Profiling] Starting perf record (freq: $PERF_SAMPLE_FREQ Hz, call-graph: fp)..."
    # `-g` = frame-pointer unwinding. The profiling image builds merod with
    # `-C force-frame-pointers=yes` under `[profile.profiling]` (debug=true,
    # strip=false) precisely so this works — `%rbp` chains give correct deep
    # stacks here, the capture is cheap, and `perf script | stackcollapse |
    # flamegraph.pl` renders cleanly against the binary's symtab. (Don't swap
    # this for `--call-graph dwarf` without also bumping `-m` and capping the
    # snapshot size, and verifying merod's .debug_* survives the release
    # pipeline — otherwise the render fails and perf thrashes its mmap ring.)
    perf record -F "$PERF_SAMPLE_FREQ" -g -p "$pid" -o "$perf_output" > "$perf_log" 2>&1 &
    PERF_PID=$!
    echo $PERF_PID > "$PROFILING_OUTPUT_DIR/perf.pid"
    
    sleep 2
    if ! kill -0 "$PERF_PID" 2>/dev/null; then
        profiling_status "perf process died immediately after spawn (PERF_PID=$PERF_PID)"
        if [ -f "$perf_log" ]; then
            # Surface perf's own first lines of stderr into the bind-mount
            # status log so we can tell "perf_event_open: Operation not
            # permitted" apart from "couldn't find binary" without docker
            # logs.
            head -5 "$perf_log" | while IFS= read -r l; do
                profiling_status "  perf-log: $l"
            done
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
        return
    fi

    profiling_status "perf recording started (PERF_PID=$PERF_PID, target=$pid, output=$perf_output)"
    
    sleep 2
    
    # Check if perf is still running
    if ! kill -0 "$PERF_PID" 2>/dev/null; then
        echo "[Profiling] ERROR: perf process died"
        if [ -f "$perf_log" ]; then
            echo "[Profiling] perf error log:"
            cat "$perf_log" | head -20
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
        return
    fi
    
    # Check process CPU usage for informational purposes
    if command -v ps >/dev/null 2>&1; then
        local cpu_usage=$(ps -p "$pid" -o %cpu= 2>/dev/null | tr -d ' ' || echo "N/A")
        echo "[Profiling] Target process CPU usage: ${cpu_usage}%"
        
        if [ "$cpu_usage" != "N/A" ] && [ -n "$cpu_usage" ]; then
            local cpu_int=$(echo "$cpu_usage" | awk -F. '{print $1}')
            if [ -n "$cpu_int" ] && [ "$cpu_int" -lt 5 ] 2>/dev/null; then
                echo "[Profiling] Note: Low CPU usage (${cpu_usage}%) may result in fewer samples. perf buffers data and writes periodically."
            fi
        fi
    fi
    
    echo "[Profiling] ✓ perf is running. Data will be collected and written periodically."
    
    # Monitor for perf.map file generation (for WASM JIT code symbolization)
    if [ "$ENABLE_WASMER_PROFILING" = "true" ]; then
        if [ -z "$pid" ]; then
            echo "[Profiling] WARNING: PID not available, cannot monitor perf.map file"
        else
            (
                echo "[Profiling] Monitoring for perf.map file generation..."
                check_count=0
                max_checks=30
                while [ $check_count -lt $max_checks ]; do
                    sleep 2
                    check_count=$((check_count + 1))
                    perf_map="/tmp/perf-${pid}.map"
                    if [ -f "$perf_map" ]; then
                        map_size=$(stat -f%z "$perf_map" 2>/dev/null || stat -c%s "$perf_map" 2>/dev/null || echo "0")
                        echo "[Profiling] ✓ perf.map file detected: $perf_map ($map_size bytes)"
                        echo "[Profiling]   This file enables WASM function name symbolization in flamegraphs"
                        break
                    fi
                done
                if [ $check_count -eq $max_checks ]; then
                    echo "[Profiling] Note: perf.map file not detected after ${max_checks} checks (60 seconds)"
                fi
            ) &
        fi
    fi
}

stop_profiling() {
    echo "[Profiling] Stopping profiling..."
    
    if [ -f "$PROFILING_OUTPUT_DIR/perf.pid" ]; then
        local perf_pid=$(cat "$PROFILING_OUTPUT_DIR/perf.pid")
        if kill -0 "$perf_pid" 2>/dev/null; then
            kill -INT "$perf_pid" 2>/dev/null || true
            
            # 15s to let perf flush its final buffer on SIGINT before SIGKILL.
            local wait_count=0
            while kill -0 "$perf_pid" 2>/dev/null && [ $wait_count -lt 15 ]; do
                sleep 1
                wait_count=$((wait_count + 1))
            done
            
            if kill -0 "$perf_pid" 2>/dev/null; then
                echo "[Profiling] WARNING: perf did not stop gracefully, forcing kill"
                kill -KILL "$perf_pid" 2>/dev/null || true
            fi
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
        
        local perf_files=$(ls "$PROFILING_OUTPUT_DIR"/perf-*.data 2>/dev/null || true)
        if [ -n "$perf_files" ]; then
            for perf_file in $perf_files; do
                local file_size=$(stat -f%z "$perf_file" 2>/dev/null || stat -c%s "$perf_file" 2>/dev/null || echo "0")
                
                if [ "$file_size" -lt 1000 ]; then
                    echo "[Profiling] WARNING: perf data file is very small: $perf_file ($file_size bytes)"
                    echo "[Profiling]   perf may have collected minimal/no samples"
                else
                    local sample_count=$(perf report -i "$perf_file" --stdio 2>/dev/null | grep -E "^# Samples:" | head -1 | awk '{print $3}' || echo "unknown")
                    
                    if [ "$sample_count" != "unknown" ] && [ "$sample_count" != "0" ]; then
                        echo "[Profiling] ✓ perf data file created: $perf_file ($file_size bytes, $sample_count samples)"
                    else
                        echo "[Profiling] perf data file exists but may be empty: $perf_file ($file_size bytes, samples: $sample_count)"
                    fi
                fi
            done
        else
            echo "[Profiling] WARNING: No perf data files found in $PROFILING_OUTPUT_DIR"
        fi
    fi
    
    # Preserve perf.map files for JIT code symbolization
    # Wasmer writes perf.map files to /tmp/perf-<pid>.map for WASM function names
    if [ "$ENABLE_WASMER_PROFILING" = "true" ]; then
        local merod_pid=$(pgrep -x merod 2>/dev/null | head -1)
        if [ -n "$merod_pid" ]; then
            local perf_map="/tmp/perf-${merod_pid}.map"
            if [ -f "$perf_map" ]; then
                local perf_map_copy="$PROFILING_OUTPUT_DIR/perf-${NODE_NAME:-merod}-${merod_pid}.map"
                echo "[Profiling] Copying perf.map file for WASM symbolization..."
                cp "$perf_map" "$perf_map_copy" 2>/dev/null || true
                if [ -f "$perf_map_copy" ]; then
                    local map_size=$(stat -f%z "$perf_map_copy" 2>/dev/null || stat -c%s "$perf_map_copy" 2>/dev/null || echo "0")
                    echo "[Profiling] ✓ perf.map file preserved: $(basename "$perf_map_copy") ($map_size bytes)"
                else
                    echo "[Profiling] WARNING: Could not copy perf.map file"
                fi
            else
                echo "[Profiling] Note: No perf.map file found at $perf_map (WASM profiling may not be active)"
            fi
        fi
    fi
}

preserve_to_host_mount() {
    # Copy /profiling/{data,reports} onto the CALIMERO_HOME bind mount so
    # it survives container removal by merobox graceful shutdown. Must not
    # fail under `set -e`: runs before `exit $EXIT_CODE` in the mainline
    # path and a non-zero return here would replace merod's real exit code.
    local host_mount="${CALIMERO_HOME:-}"
    if [ -z "$host_mount" ]; then
        echo "[Profiling] CALIMERO_HOME unset, skipping preserve"
        return 0
    fi
    if [ ! -d "$host_mount" ]; then
        echo "[Profiling] CALIMERO_HOME=$host_mount not a directory, skipping preserve"
        return 0
    fi
    local err_file
    err_file=$(mktemp -t preserve.err.XXXXXX 2>/dev/null) || err_file=/dev/null
    local dest="$host_mount/profiling-dump"
    if ! mkdir -p "$dest" 2>"$err_file"; then
        echo "[Profiling] WARNING: could not create $dest: $(head -1 "$err_file" 2>/dev/null)"
        [ "$err_file" != /dev/null ] && rm -f "$err_file"
        return 0
    fi
    if [ -d "$PROFILING_OUTPUT_DIR" ]; then
        if ! cp -r "$PROFILING_OUTPUT_DIR/." "$dest/" 2>"$err_file"; then
            echo "[Profiling] WARNING: cp from $PROFILING_OUTPUT_DIR may be incomplete: $(head -3 "$err_file" 2>/dev/null | tr '\n' ' ')"
        fi
    fi

    local node_name="${NODE_NAME:-merod}"
    local dest_reports="$dest/reports"
    if ! mkdir -p "$dest_reports" 2>"$err_file"; then
        echo "[Profiling] WARNING: could not create $dest_reports: $(head -1 "$err_file" 2>/dev/null)"
    fi

    # Write flamegraphs directly to the bind mount so they survive even if
    # a subsequent copy step fails.
    local perf_data="$PROFILING_OUTPUT_DIR/perf-${node_name}.data"
    if [ -d "$dest_reports" ] && [ -f "$perf_data" ] && [ -s "$perf_data" ] && command -v perf >/dev/null 2>&1; then
        local cpu_svg="$dest_reports/flamegraph-cpu-${node_name}.svg"
        if /profiling/scripts/generate-flamegraph.sh \
            --input "$perf_data" \
            --output "$cpu_svg" \
            --title "CPU Flamegraph - ${node_name}" \
            >/dev/null 2>"$err_file"; then
            echo "[Profiling] ✓ CPU flamegraph -> $cpu_svg"
        else
            echo "[Profiling] WARNING: CPU flamegraph failed: $(head -1 "$err_file" 2>/dev/null)"
        fi
    fi

    # Match generate-memory-flamegraph.sh's own glob (no dot between
    # "jemalloc" and the PID) so the guard and the script agree.
    if [ -d "$dest_reports" ] && ls "$PROFILING_OUTPUT_DIR"/jemalloc*.heap >/dev/null 2>&1; then
        local mem_svg="$dest_reports/flamegraph-memory-${node_name}.svg"
        if /profiling/scripts/generate-memory-flamegraph.sh \
            --latest \
            --input-dir "$PROFILING_OUTPUT_DIR" \
            --output "$mem_svg" \
            --title "Memory Flamegraph - ${node_name}" \
            --colors mem \
            >/dev/null 2>"$err_file"; then
            echo "[Profiling] ✓ memory flamegraph -> $mem_svg"
        else
            echo "[Profiling] WARNING: memory flamegraph failed: $(head -1 "$err_file" 2>/dev/null)"
        fi
    fi

    # Also pick up any pre-existing files a caller might have placed under
    # $PROFILING_REPORTS_DIR (e.g. older scripts). No-op in the common case.
    local reports_dir="${PROFILING_REPORTS_DIR:-/profiling/reports}"
    if [ -d "$reports_dir" ] && [ -d "$dest_reports" ]; then
        if ! cp -r "$reports_dir/." "$dest_reports/" 2>"$err_file"; then
            echo "[Profiling] WARNING: cp from $reports_dir may be incomplete: $(head -3 "$err_file" 2>/dev/null | tr '\n' ' ')"
        fi
    fi
    # perf record writes perf-*.data with mode 600, so the unprivileged
    # runner user doing host-side harvest can't read it. go+rX adds
    # group/other read + conditional-execute for dirs only.
    chmod -R go+rX "$dest" 2>"$err_file" || {
        echo "[Profiling] WARNING: chmod on $dest failed: $(head -1 "$err_file" 2>/dev/null)"
    }
    [ "$err_file" != /dev/null ] && rm -f "$err_file"
    local size
    size=$(du -sh "$dest" 2>/dev/null | awk '{print $1}')
    echo "[Profiling] ✓ Preserved profiling data to $dest (${size:-unknown size})"
    return 0
}

start_periodic_preserve_loop() {
    # Background loop that runs `preserve_to_host_mount` on a cadence
    # (default 60s) for the whole life of the container. Why:
    #
    # The previous design only preserved on shutdown via the SIGTERM trap.
    # Docker's stop-timeout (default 10s) often killed the container
    # before the trap finished — `stop_profiling` waits up to 15s for
    # perf to flush, then the merod-stop wait runs up to 10s, *then*
    # `preserve_to_host_mount` rendered flamegraphs. By that point
    # SIGKILL had already arrived, the bind mount got partial data
    # (jemalloc heap files, no perf data on some nodes, no `reports/`
    # directory at all on any node — see the
    # `profiling-data-group-governance-499-25799459525` artifact).
    #
    # With this loop, the bind mount holds an at-most-${PRESERVE_INTERVAL}-
    # stale copy of `/profiling/data/.` + a freshly-rendered flamegraph
    # at all times. A truncated shutdown loses at most one interval's
    # worth of samples, not the whole run.
    #
    # Cost: `cp -r` of ~50MB jemalloc dumps + a flamegraph render every
    # 60s. The cp is fast (small files, same filesystem). The render is
    # the expensive part — `perf script | stackcollapse | flamegraph.pl`
    # on a 1.9MB perf.data takes ~2-3s. Both well under interval.
    if [ "${PRESERVE_INTERVAL_SECONDS:-0}" -le 0 ]; then
        profiling_status "periodic preserve disabled (PRESERVE_INTERVAL_SECONDS=0)"
        return
    fi
    if [ -z "${CALIMERO_HOME:-}" ] || [ ! -d "${CALIMERO_HOME:-}" ]; then
        profiling_status "CALIMERO_HOME unset or not a dir, skipping periodic preserve"
        return
    fi

    (
        # Run preserve_to_host_mount on the cadence. Each call is
        # idempotent and overwrites the previous artifacts in place.
        # Detached from the controlling terminal so a stray SIGINT to
        # the main shell doesn't kill the loop separately from the
        # `kill $PRESERVE_LOOP_PID` in cleanup().
        while sleep "$PRESERVE_INTERVAL_SECONDS"; do
            # `set -e` in the parent doesn't propagate to subshells, so a
            # transient preserve failure here doesn't crash the loop —
            # just retry on next tick.
            preserve_to_host_mount >/dev/null 2>&1 || true
        done
    ) &
    PRESERVE_LOOP_PID=$!
    profiling_status "periodic preserve loop started (PID=$PRESERVE_LOOP_PID, interval=${PRESERVE_INTERVAL_SECONDS}s)"
}

cleanup() {
    local signal_exit_code=$?
    echo "[Profiling] Received signal, cleaning up..."

    # Stop the periodic preserve loop first so its in-flight `cp`/render
    # can't race the final `preserve_to_host_mount` call below. Even if
    # this kill arrives mid-cp, the previous-tick artifacts on the bind
    # mount are still good — that's the whole point of the loop.
    if [ -n "$PRESERVE_LOOP_PID" ] && kill -0 "$PRESERVE_LOOP_PID" 2>/dev/null; then
        kill -TERM "$PRESERVE_LOOP_PID" 2>/dev/null || true
        wait "$PRESERVE_LOOP_PID" 2>/dev/null || true
    fi

    stop_profiling

    if [ -n "$MAIN_PID" ] && kill -0 "$MAIN_PID" 2>/dev/null; then
        echo "[Profiling] Stopping main process (PID: $MAIN_PID)..."
        kill -TERM "$MAIN_PID" 2>/dev/null || true

        local wait_count=0
        while kill -0 "$MAIN_PID" 2>/dev/null && [ $wait_count -lt 10 ]; do
            sleep 1
            wait_count=$((wait_count + 1))
        done

        if kill -0 "$MAIN_PID" 2>/dev/null; then
            kill -KILL "$MAIN_PID" 2>/dev/null || true
        fi
    fi

    preserve_to_host_mount

    if [ "$EXIT_CODE" -ne 0 ]; then
        exit $EXIT_CODE
    elif [ "$signal_exit_code" -ne 0 ]; then
        exit $signal_exit_code
    else
        exit 143
    fi
}

trap cleanup SIGTERM SIGINT

detect_jemalloc_path() {
    if [ -f "/usr/local/lib/libjemalloc.so.2" ]; then
        echo "/usr/local/lib/libjemalloc.so.2"
        return
    fi
    local arch=$(uname -m)
    case "$arch" in
        x86_64)    echo "/usr/lib/x86_64-linux-gnu/libjemalloc.so.2" ;;
        aarch64)   echo "/usr/lib/aarch64-linux-gnu/libjemalloc.so.2" ;;
        *)         echo "" ;;
    esac
}

if [ "$ENABLE_PERF" = "true" ]; then
    install_kernel_tools || true
fi

# Resolve jemalloc preload path but DON'T export it. Globally exporting
# LD_PRELOAD causes every helper this script and preserve_to_host_mount
# spawn (mkdir, cp, mktemp, chmod, awk…) to also load libjemalloc and —
# combined with `prof_final:true` in MALLOC_CONF — write its own
# `jemalloc.<helper-pid>.<seq>.f.heap` on exit. Those tiny dumps end up
# newer than merod's own dumps, so `generate-memory-flamegraph.sh
# --latest` (`ls -t … | head -1`) ends up rendering a flamegraph from a
# helper's heap whose MAPPED_LIBRARIES point at e.g. /usr/bin/mkdir, not
# /usr/local/bin/merod — making jeprof's symbolisation of app frames
# fall through to its `[$prog, 0, max_pc, 0]` catch-all and emit raw
# hex addresses instead of function names.
JEMALLOC_LD_PRELOAD=""
if [ "$ENABLE_JEMALLOC" = "true" ]; then
    JEMALLOC_PATH="${LD_PRELOAD_JEMALLOC:-$(detect_jemalloc_path)}"
    if [ -n "$JEMALLOC_PATH" ] && [ -f "$JEMALLOC_PATH" ]; then
        JEMALLOC_LD_PRELOAD="$JEMALLOC_PATH"
        echo "[Profiling] jemalloc enabled (LD_PRELOAD=$JEMALLOC_LD_PRELOAD, scoped to main process only)"
        if [[ "$JEMALLOC_PATH" == "/usr/local/lib/"* ]]; then
            echo "[Profiling] Using source-built jemalloc with profiling support"
        fi
    else
        echo "[Profiling] jemalloc library not found, skipping"
    fi
fi

if [ "$ENABLE_HEAPTRACK" = "true" ]; then
    HEAPTRACK_OUTPUT="$PROFILING_OUTPUT_DIR/heaptrack-${NODE_NAME:-merod}"
    set -- heaptrack -o "$HEAPTRACK_OUTPUT" "$@"
    echo "[Profiling] heaptrack enabled (output: $HEAPTRACK_OUTPUT)"
fi

echo "[Profiling] Executing: $@"

SHOULD_PROFILE_WITH_PERF=true
for arg in "$@"; do
    if [[ "$arg" == "init" ]] || [[ "$arg" == "--help" ]] || [[ "$arg" == "-h" ]]; then
        SHOULD_PROFILE_WITH_PERF=false
        echo "[Profiling] Skipping perf profiling for short-lived command: $arg"
        break
    fi
done

if [ "$ENABLE_PROFILING" = "true" ] && [ "$ENABLE_PERF" = "true" ] && [ "$SHOULD_PROFILE_WITH_PERF" = "true" ]; then
    LD_PRELOAD="$JEMALLOC_LD_PRELOAD" "$@" &
    MAIN_PID=$!
    echo "[Profiling] Process started with PID $MAIN_PID"
    
    sleep 3
    
    if ! kill -0 "$MAIN_PID" 2>/dev/null; then
        echo "[Profiling] Process already exited, skipping perf profiling"
        wait $MAIN_PID
        EXIT_CODE=$?
        exit $EXIT_CODE
    fi
    
    if [ "$ENABLE_HEAPTRACK" = "true" ]; then
        ACTUAL_PID=$(pgrep -P "$MAIN_PID" 2>/dev/null | head -1 || echo "$MAIN_PID")
        if [ "$ACTUAL_PID" != "$MAIN_PID" ]; then
            echo "[Profiling] Found child process: PID $ACTUAL_PID"
        fi
        PERF_TARGET_PID=$ACTUAL_PID
    else
        PERF_TARGET_PID=$MAIN_PID
    fi
    
    start_profiling $PERF_TARGET_PID

    # Spin the periodic-preserve loop AFTER perf is recording, so the
    # first tick has fresh perf data to render. Loop runs until cleanup()
    # kills it; even if Docker SIGKILLs us mid-trap, the bind mount holds
    # an at-most-${PRESERVE_INTERVAL_SECONDS}-stale flamegraph.
    start_periodic_preserve_loop

    wait $MAIN_PID
    EXIT_CODE=$?

    stop_profiling
    preserve_to_host_mount
    exit $EXIT_CODE
else
    exec env LD_PRELOAD="$JEMALLOC_LD_PRELOAD" "$@"
fi