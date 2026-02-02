#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: run-merobox-workflows.sh --root <dir> --workflow-list <file> --image <image> \
  --contracts-dir <dir> --docker-logs <dir> --workflow-logs <dir> --failed-output <file> \
  [--max-attempts <n>] [--workdir-mode root|parent]

Options:
  --root           Root directory for workflow execution.
  --workflow-list  File containing workflow paths (one per line, relative to root).
  --image          Merod image tag to use.
  --contracts-dir  Contracts directory for --near-devnet.
  --docker-logs    Directory to write docker logs.
  --workflow-logs  Directory to write per-workflow stdout logs.
  --failed-output  Output file for failed workflow list.
  --max-attempts   Retry attempts per workflow (default: 2).
  --workdir-mode   "root" to run from root, "parent" to run from workflow parent.
EOF
}

root=""
workflow_list=""
image=""
contracts_dir=""
docker_logs=""
workflow_logs=""
failed_output=""
max_attempts=2
workdir_mode="root"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --root)
      root="$2"
      shift 2
      ;;
    --workflow-list)
      workflow_list="$2"
      shift 2
      ;;
    --image)
      image="$2"
      shift 2
      ;;
    --contracts-dir)
      contracts_dir="$2"
      shift 2
      ;;
    --docker-logs)
      docker_logs="$2"
      shift 2
      ;;
    --workflow-logs)
      workflow_logs="$2"
      shift 2
      ;;
    --failed-output)
      failed_output="$2"
      shift 2
      ;;
    --max-attempts)
      max_attempts="$2"
      shift 2
      ;;
    --workdir-mode)
      workdir_mode="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1"
      usage
      exit 2
      ;;
  esac
done

if [[ -z "$root" || -z "$workflow_list" || -z "$image" || -z "$contracts_dir" || -z "$docker_logs" || -z "$workflow_logs" || -z "$failed_output" ]]; then
  echo "Missing required arguments."
  usage
  exit 2
fi

if [[ ! -f "$workflow_list" ]]; then
  echo "Workflow list not found: $workflow_list"
  exit 2
fi

mkdir -p "$docker_logs" "$workflow_logs"

failed_workflows=()

filter_container_names() {
  if command -v rg >/dev/null 2>&1; then
    docker ps -a --format "{{.Names}}" 2>/dev/null | rg -i "calimero|merobox|node" || true
  else
    docker ps -a --format "{{.Names}}" 2>/dev/null | grep -Ei "calimero|merobox|node" || true
  fi
}

sanitize_workflow_id() {
  local workflow="$1"
  local id="${workflow//\//-}"
  id="${id%.yml}"
  id="${id%.yaml}"
  echo "$id"
}

while IFS= read -r workflow || [[ -n "$workflow" ]]; do
  [[ -z "$workflow" ]] && continue

  if [[ "$workdir_mode" == "parent" ]]; then
    app_dir=$(dirname "$(dirname "$workflow")")
    workflow_arg="workflows/$(basename "$workflow")"
    workdir="$root/$app_dir"
  else
    workflow_arg="$workflow"
    workdir="$root"
  fi

  if [[ ! -f "$workdir/$workflow_arg" ]]; then
    echo "Workflow not found: $workdir/$workflow_arg"
    failed_workflows+=("$workflow")
    continue
  fi

  workflow_id=$(sanitize_workflow_id "$workflow")
  success=false
  attempt=1

  while [[ $attempt -le $max_attempts ]]; do
    workflow_log="$workflow_logs/${workflow_id}-attempt-${attempt}.log"

    if [[ $attempt -gt 1 ]]; then
      merobox stop --all || true
      merobox nuke -f || true
      sleep 2
    fi

    if (
      cd "$workdir"
      merobox bootstrap run "$workflow_arg" \
        --image "$image" \
        --e2e-mode \
        --near-devnet \
        --contracts-dir "$contracts_dir" \
        --verbose
    ) 2>&1 | tee "$workflow_log"; then
      success=true
      break
    else
      attempt=$((attempt + 1))
    fi
  done

  if [[ "$success" == "false" ]]; then
    failed_workflows+=("$workflow")
  fi

  echo "Collecting Docker logs for workflow: $workflow (id: $workflow_id)"
  containers=$(docker ps -a --filter "label=calimero.node=true" --format "{{.Names}}" 2>/dev/null || true)
  if [[ -z "$containers" ]]; then
    containers=$(filter_container_names)
  fi
  if [[ -z "$containers" ]]; then
    containers=$(docker ps -a --format "{{.Names}}" 2>/dev/null || true)
  fi

  if [[ -n "$containers" ]]; then
    while IFS= read -r container; do
      if [[ -n "$container" ]]; then
        echo "Collecting logs from container: $container"
        docker logs "$container" > "$docker_logs/${workflow_id}-${container}.log" 2>&1 || true
      fi
    done <<< "$containers"
  fi

  merobox stop --all || true
  merobox nuke -f || true
done < "$workflow_list"

if [[ ${#failed_workflows[@]} -gt 0 ]]; then
  printf '%s\n' "${failed_workflows[@]}" > "$failed_output"
else
  rm -f "$failed_output"
fi
