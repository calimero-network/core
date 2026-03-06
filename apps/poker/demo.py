#!/usr/bin/env python3
"""
♠♥♣♦  CALIMERO POKER — SECURE DEALING DEMO  ♦♣♥♠

3-player poker with commit-reveal shuffle + encrypted cards.
Single-process driver that controls everything via JSON-RPC.

Usage:
  python3 demo.py                    # default 1s pacing
  python3 demo.py --pace 0.5         # fast
  python3 demo.py --pace 2           # slow
  python3 demo.py --max-hands 10     # limit hands
"""

import argparse
import hashlib
import json
import os
import random
import re
import subprocess
import sys
import time

# ── Colors ──────────────────────────────────────────────────
class C:
    W = "\033[1;37m"   # white bold
    R = "\033[0;31m"   # red
    G = "\033[0;32m"   # green
    Y = "\033[0;33m"   # yellow
    B = "\033[0;34m"   # blue
    M = "\033[0;35m"   # magenta
    CY = "\033[0;36m"  # cyan
    DIM = "\033[2m"    # dim
    X = "\033[0m"      # reset

# ── Players ─────────────────────────────────────────────────
PLAYERS = [
    {"name": "🦈 SHARK",   "strat": 2, "color": C.Y},
    {"name": "📞 STATION", "strat": 1, "color": C.G},
    {"name": "🎲 GAMBLER", "strat": 0, "color": C.CY},
]

# ── RPC ─────────────────────────────────────────────────────
class RPC:
    def __init__(self, port, ctx, key):
        self.url = f"http://localhost:{port}/jsonrpc"
        self.ctx = ctx
        self.key = key

    def call(self, method, args=None):
        body = {
            "jsonrpc": "2.0", "id": "1", "method": "execute",
            "params": {
                "contextId": self.ctx, "method": method,
                "argsJson": args or {}, "executorPublicKey": self.key,
                "substitute": [],
            },
        }
        try:
            import urllib.request
            req = urllib.request.Request(
                self.url, data=json.dumps(body).encode(),
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=15) as resp:
                data = json.loads(resp.read())
            if "error" in data:
                return None
            return data.get("result", {}).get("output")
        except Exception:
            return None

    def sync_trigger(self):
        try:
            import urllib.request
            url = f"http://localhost:{self.url.split(':')[2].split('/')[0]}/admin-api/contexts/sync/{self.ctx}"
            req = urllib.request.Request(url, method="POST")
            urllib.request.urlopen(req, timeout=5)
        except Exception:
            pass

# ── Helpers ──────────────────────────────────────────────────
def seed_hash(seed_bytes):
    """Match crypto::hash_seed — prepends 'calimero-poker-seed:'"""
    h = hashlib.sha256(b"calimero-poker-seed:" + seed_bytes).digest()
    return list(h)

def pname(key, players_map):
    info = players_map.get(key[:8])
    return info["name"] if info else key[:8]

def pcolor(key, players_map):
    info = players_map.get(key[:8])
    return info["color"] if info else ""

def wait_sync(rpcs, pace):
    """Trigger sync on all nodes and wait."""
    for r in rpcs:
        r.sync_trigger()
    time.sleep(max(pace * 2, 2))

def print_bar(chips, max_chips=600):
    width = min(int(chips / max_chips * 25), 25)
    return "█" * width

# ── Setup ────────────────────────────────────────────────────
def setup(merod_path, script_dir):
    """Run merobox to create 3-node table. Returns (ctx, keys, ports)."""
    yml = os.path.join(script_dir, ".demo-secure-setup.yml")
    num_bots = len(PLAYERS)
    num_nodes = num_bots + 1  # +1 for dealer
    bot_nodes = ", ".join(f"demo-{i}" for i in range(2, num_nodes + 1))

    with open(yml, "w") as f:
        f.write(f"""name: Setup
force_pull_image: true
nodes:
  chain_id: testnet-1
  count: {num_nodes}
  image: ghcr.io/calimero-network/merod:edge
  prefix: demo
steps:
  - {{name: Install, type: install_application, node: demo-1, path: res/poker.wasm, dev: true, outputs: {{app_id: applicationId}}}}
  - {{name: Mesh, type: create_mesh, context_node: demo-1, application_id: "{{{{app_id}}}}", params: '{{"small_blind":5,"big_blind":10,"min_buy_in":50}}', nodes: [{bot_nodes}], capability: member, outputs: {{context_id: contextId, member_public_key: memberPublicKey}}}}
  - {{name: READY, type: wait, seconds: 600}}
stop_all_nodes: false
wait_timeout: 700
""")

    log_path = "/tmp/_poker_demo.log"
    proc = subprocess.Popen(
        ["merobox", "bootstrap", "run", yml,
         "--no-docker", "--binary-path", merod_path,
         "--e2e-mode", "-v"],
        stdout=open(log_path, "w"), stderr=subprocess.STDOUT,
        cwd=script_dir,
    )

    # Wait for READY
    for _ in range(120):
        try:
            with open(log_path) as f:
                log = f.read()
            if "READY" in log:
                break
        except Exception:
            pass
        time.sleep(1)
    else:
        print("Setup timed out. Check /tmp/_poker_demo.log")
        sys.exit(1)

    with open(log_path) as f:
        log = f.read()

    ctx = re.search(r"context_id = (\S+)", log).group(1)
    dealer_key = re.search(r"member_public_key = (\S+)", log).group(1)
    bot_keys = re.findall(r"Identity created: (\S+)", log)
    ports = [int(p) for p in re.findall(r"RPC port: (\d+)", log)]

    return proc, ctx, dealer_key, bot_keys, ports

# ── Spectator ────────────────────────────────────────────────
def launch_spectator(script_dir, port, node_port, ctx, key):
    """Start HTTP server and open spectator UI with pre-filled config."""
    import http.server
    import threading
    import webbrowser
    import urllib.parse

    class QuietHandler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=script_dir, **kw)
        def log_message(self, format, *a):
            pass  # silence logs

    server = http.server.HTTPServer(("", port), QuietHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    # Open browser with pre-filled URL params
    params = urllib.parse.urlencode({"node": f"http://localhost:{node_port}", "ctx": ctx, "key": key})
    url = f"http://localhost:{port}/spectator.html?{params}"
    print(f"{C.CY}  🌐 Spectator: {url}{C.X}")
    webbrowser.open(url)

# ── Main ─────────────────────────────────────────────────────
def main():
    parser = argparse.ArgumentParser(description="Calimero Poker Demo")
    parser.add_argument("--pace", type=float, default=1.0, help="Seconds between actions")
    parser.add_argument("--max-hands", type=int, default=999, help="Max hands to play")
    parser.add_argument("--merod", type=str, default=None, help="Path to merod binary (default: target/release/merod)")
    parser.add_argument("--spectator", action="store_true", help="Launch spectator UI in browser")
    parser.add_argument("--spectator-port", type=int, default=8080, help="Port for spectator HTTP server")
    args = parser.parse_args()

    script_dir = os.path.dirname(os.path.abspath(__file__))
    root = os.path.join(script_dir, "../..")
    merod = args.merod or os.path.join(root, "target/release/merod")

    # Banner
    print()
    print(f"{C.W}╔══════════════════════════════════════════════════╗{C.X}")
    print(f"{C.W}║{C.R} ♠{C.Y}♥{C.G}♣{C.CY}♦{C.X}  {C.W}CALIMERO POKER — SECURE DEALING{C.X}       {C.W}║{C.X}")
    print(f"{C.W}║{C.X}  🔒 Commit-reveal shuffle + encrypted cards      {C.W}║{C.X}")
    for p in PLAYERS:
        print(f"{C.W}║{C.X}  {p['name']}                                        {C.W}║{C.X}")
    print(f"{C.W}║{C.X}  Buy-in: 200  Blinds: 5/10 → escalating          {C.W}║{C.X}")
    print(f"{C.W}╚══════════════════════════════════════════════════╝{C.X}")
    print()

    num_bots = len(PLAYERS)
    print(f"{C.DIM}Setting up {num_bots + 1}-node table ({num_bots} bots + dealer)...{C.X}", flush=True)
    mero_proc, ctx, dealer_key, bot_keys, ports = setup(merod, script_dir)

    dealer_port = ports[0]
    dealer = RPC(dealer_port, ctx, dealer_key)
    bot_rpcs = []
    all_rpcs = [dealer]
    players_map = {
        dealer_key[:8]: {"name": "🎰 DEALER", "strat": -1, "color": C.DIM, "rpc": dealer, "key": dealer_key},
    }

    for i, player_info in enumerate(PLAYERS):
        key = bot_keys[i]
        port = ports[i + 1]  # dealer is port[0], bots are port[1..]
        rpc = RPC(port, ctx, key)
        bot_rpcs.append(rpc)
        all_rpcs.append(rpc)
        players_map[key[:8]] = {**player_info, "rpc": rpc, "key": key}

    print(f"{C.G}✓ Table ready{C.X}")

    # ── One-time setup: secure mode + keys + join ──
    print(f"{C.DIM}  Enabling secure mode...{C.X}", flush=True)
    dealer.call("enable_secure_mode")
    dealer.call("register_dealer")
    wait_sync(all_rpcs, args.pace)

    for rpc, key in zip(bot_rpcs, bot_keys):
        rpc.call("register_encryption_key")
        rpc.call("join_table", {"buy_in": 200})
        wait_sync(all_rpcs, args.pace)

    print(f"{C.G}✓ Players seated, keys registered{C.X}")

    # Launch spectator UI if requested
    if args.spectator:
        launch_spectator(script_dir, args.spectator_port, dealer_port, ctx, dealer_key)

    print()

    # ── Game loop ────────────────────────────────────────
    blind_level = 0
    sb, bb = 5, 10

    try:
        for hand_num in range(1, args.max_hands + 1):
            # Blind escalation
            new_level = (hand_num - 1) // 5
            if new_level > blind_level:
                blind_level = new_level
                sb, bb = [(5,10),(10,20),(25,50),(50,100),(100,200)][min(new_level, 4)]
                dealer.call("configure", {"small_blind": sb, "big_blind": bb})
                wait_sync(all_rpcs, args.pace)
                print(f"  {C.Y}⬆ BLINDS UP: {sb}/{bb}{C.X}")
                print()

            # ── Commit-Reveal ──
            seeds = {}
            for rpc, key in zip(bot_rpcs, bot_keys):
                seed = os.urandom(16)
                seeds[key] = seed
                h = seed_hash(seed)
                rpc.call("commit_seed", {"seed_hash": h})
            wait_sync(all_rpcs, args.pace)

            for rpc, key in zip(bot_rpcs, bot_keys):
                rpc.call("reveal_seed", {"seed": list(seeds[key])})
            wait_sync(all_rpcs, args.pace)

            # ── Dealer deals ──
            dealer.call("dealer_deal")
            wait_sync(all_rpcs, args.pace)

            state = dealer.call("get_game_state")
            if not state or state.get("phase") != "PreFlop":
                print(f"  {C.R}Deal failed, skipping hand{C.X}")
                continue

            pot = state["pot"]
            print(f"{C.W}{'─'*50}{C.X}")
            print(f"{C.W}  HAND #{hand_num}{C.X}  {C.DIM}│ Blinds {sb}/{bb} │ Pot: {pot}{C.X}")
            print(f"{C.W}{'─'*50}{C.X}")

            # Get each bot's cards (only they can see their own)
            my_cards = {}
            for rpc, key in zip(bot_rpcs, bot_keys):
                cards = rpc.call("get_my_cards_secure") or []
                my_cards[key] = cards

            last_phase = ""

            # ── Play hand ──
            while True:
                state = dealer.call("get_game_state")
                if not state:
                    time.sleep(1)
                    continue
                phase = state.get("phase", "")

                if phase == "Waiting":
                    break

                # Phase header
                if phase != last_phase:
                    community = " ".join(state.get("community_cards", []))
                    pot = state.get("pot", 0)
                    print()
                    if phase == "PreFlop":
                        print(f"  {C.W}PREFLOP{C.X}  {C.DIM}pot: {pot}{C.X}")
                    elif phase == "Flop":
                        print(f"  {C.W}FLOP{C.X}    {C.CY}[ {community} ]{C.X}  {C.DIM}pot: {pot}{C.X}")
                    elif phase == "Turn":
                        print(f"  {C.W}TURN{C.X}    {C.CY}[ {community} ]{C.X}  {C.DIM}pot: {pot}{C.X}")
                    elif phase == "River":
                        print(f"  {C.W}RIVER{C.X}   {C.CY}[ {community} ]{C.X}  {C.DIM}pot: {pot}{C.X}")
                    last_phase = phase

                action_on = state.get("action_on", "")
                if not action_on or action_on == "" or len(action_on) < 8:
                    # Waiting for dealer to reveal next street
                    if phase in ("PreFlop", "Flop", "Turn"):
                        # Round complete — dealer reveals next street
                        wait_sync(all_rpcs, args.pace)
                        if phase == "PreFlop":
                            dealer.call("dealer_reveal_flop")
                        elif phase == "Flop":
                            dealer.call("dealer_reveal_turn")
                        elif phase == "Turn":
                            dealer.call("dealer_reveal_river")
                        wait_sync(all_rpcs, args.pace)
                    else:
                        time.sleep(0.5)
                    continue

                # Find acting bot
                acting_rpc = None
                acting_key = None
                for rpc, key in zip(bot_rpcs, bot_keys):
                    if key == action_on:
                        acting_rpc = rpc
                        acting_key = key
                        break

                if not acting_rpc:
                    time.sleep(0.5)
                    continue

                # Wait for acting node to see it's their turn
                for _ in range(15):
                    s2 = acting_rpc.call("get_game_state")
                    if s2 and s2.get("action_on") == acting_key:
                        break
                    time.sleep(1)

                # Bot acts
                strat = players_map[acting_key[:8]]["strat"]
                result = acting_rpc.call("bot_play", {"strategy": strat})
                name = pname(acting_key, players_map)
                color = pcolor(acting_key, players_map)
                cards_str = " ".join(my_cards.get(acting_key, ["??", "??"]))
                other_cards = f"{C.DIM}[{cards_str}]{C.X}"

                if result:
                    action = result.upper()
                    if "FOLD" in action:
                        icon = f"{C.R}✗{C.X}"
                        action_fmt = f"{C.R}{action}{C.X}"
                    elif "CHECK" in action:
                        icon = f"{C.DIM}─{C.X}"
                        action_fmt = f"{action}"
                    elif "CALL" in action:
                        icon = f"{C.G}→{C.X}"
                        action_fmt = f"{C.G}{action}{C.X}"
                    elif "RAISE" in action:
                        icon = f"{C.Y}▲{C.X}"
                        action_fmt = f"{C.Y}{action}{C.X}"
                    else:
                        icon = " "
                        action_fmt = action

                    print(f"    {icon} {color}{name:<12}{C.X} {other_cards}  {action_fmt}")
                    time.sleep(args.pace)

                wait_sync(all_rpcs, args.pace * 0.5)

            # ── Showdown ──
            wait_sync(all_rpcs, args.pace)
            result = dealer.call("get_hand_result")
            if result:
                winner = result.get("winner_id", "")
                win_hand = result.get("winning_hand", "")
                pot = result.get("pot", 0)
                reason = result.get("reason", "")
                community = " ".join(result.get("community_cards", []))
                winner_name = pname(winner, players_map)

                print()
                print(f"  {C.W}┌─ SHOWDOWN ──────────────────────────────┐{C.X}")
                if reason == "showdown":
                    print(f"  {C.W}│{C.X}  Board: {C.CY}{community}{C.X}")
                    for pc in result.get("player_cards", []):
                        pid = pc["player_id"]
                        marker = "🏆" if pid == winner else "  "
                        pn = pname(pid, players_map)
                        print(f"  {C.W}│{C.X}  {marker} {pn:<12} {pc['card1']} {pc['card2']}")
                else:
                    print(f"  {C.W}│{C.X}  Everyone else folded")
                print(f"  {C.W}│{C.X}")
                print(f"  {C.W}│{C.X}  {C.G}🏆 {winner_name} wins {pot} chips{C.X}  {C.DIM}({win_hand}){C.X}")
                print(f"  {C.W}└─────────────────────────────────────────┘{C.X}")

            # Scoreboard
            stats = dealer.call("get_stats")
            if stats:
                print()
                print(f"  {C.W}SCOREBOARD{C.X}")
                players_sorted = sorted(stats.get("players", []), key=lambda p: p["chips"], reverse=True)
                alive = 0
                for p in players_sorted:
                    pid = p["player_id"]
                    chips = p["chips"]
                    wins = p["wins"]
                    name = pname(pid, players_map)
                    color = pcolor(pid, players_map)
                    bar = print_bar(chips)
                    dead = "💀 " if chips == 0 else "   "
                    print(f"    {dead}{color}{name:<12}{C.X} {chips:>4} chips  W:{wins:<2}  {bar}")
                    if chips > 0:
                        alive += 1

                total = sum(p["chips"] for p in players_sorted)
                print(f"    {C.DIM}Total: {total}  Alive: {alive}/{len(players_sorted)}{C.X}")
                print()

                if alive <= 1:
                    champ = next((p for p in players_sorted if p["chips"] > 0), None)
                    if champ:
                        cn = pname(champ["player_id"], players_map)
                        print()
                        print(f"{C.Y}  ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★{C.X}")
                        print(f"{C.W}  ║  🏆  CHAMPION: {cn}  after {hand_num} hands  ║{C.X}")
                        print(f"{C.Y}  ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★ ★{C.X}")
                    break

            time.sleep(args.pace)

    except KeyboardInterrupt:
        print(f"\n{C.DIM}Interrupted.{C.X}")

    # Cleanup
    print(f"\n{C.DIM}Cleaning up...{C.X}")
    mero_proc.terminate()
    mero_proc.wait()
    subprocess.run(["merobox", "nuke"], capture_output=True, cwd=script_dir)

if __name__ == "__main__":
    main()
