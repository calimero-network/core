//! # poker-bot — AI client for Calimero Poker

mod bot;
mod client;
mod types;

use std::thread;
use std::time::Duration;

use clap::Parser;

use bot::{Action, CallerBot, PokerBot, RandomBot, TagBot};
use client::PokerClient;

#[derive(Parser)]
#[command(name = "poker-bot", about = "AI poker player for Calimero")]
struct Cli {
    #[arg(long)]
    node: String,
    #[arg(long)]
    context: String,
    #[arg(long)]
    key: String,
    #[arg(long, default_value = "tag")]
    strategy: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long, default_value = "1000")]
    buy_in: u64,
    #[arg(long, default_value = "1000")]
    poll_ms: u64,
    /// This bot prints hand results and scoreboards (only one bot should have this)
    #[arg(long, default_value = "false")]
    reporter: bool,
}

fn main() {
    let cli = Cli::parse();
    let client = PokerClient::new(&cli.node, &cli.context, &cli.key);

    let mut bot: Box<dyn PokerBot> = match cli.strategy.as_str() {
        "caller" => Box::new(CallerBot),
        "random" => Box::new(RandomBot),
        "tag" => Box::new(TagBot),
        other => {
            eprintln!("Unknown strategy: {other}");
            std::process::exit(1);
        }
    };

    let tag = cli.name.unwrap_or_else(|| bot.name().to_uppercase());

    if cli.buy_in > 0 {
        let _ = client.join_table(cli.buy_in);
    }

    let poll = Duration::from_millis(cli.poll_ms);
    let my_key = cli.key.clone();
    let mut last_hand: u64 = 0;
    let mut last_phase = String::new();

    loop {
        thread::sleep(poll);

        let state = match client.get_game_state() {
            Ok(s) => s,
            Err(_) => continue,
        };

        // ── Hand result (only reporter prints) ──
        if state.hand_number > last_hand && last_hand > 0 && cli.reporter {
            if let Ok(r) = client.get_hand_result() {
                println!();
                println!(
                    "  ┌─ HAND #{} RESULT ─────────────────────────┐",
                    r.hand_number
                );
                println!("  │  Board: {}", r.community_cards.join(" "));
                for pc in &r.player_cards {
                    let m = if pc.player_id == r.winner_id {
                        "🏆"
                    } else {
                        "  "
                    };
                    println!(
                        "  │  {} {}  {} {}",
                        m,
                        &pc.player_id[..8],
                        pc.card1,
                        pc.card2
                    );
                }
                println!(
                    "  │  Winner: {} — {} ({})",
                    &r.winner_id[..8],
                    r.winning_hand,
                    r.pot
                );
                println!("  │");

                if let Ok(stats) = client.get_stats() {
                    for p in &stats.players {
                        let bar: String = "█".repeat((p.chips / 20) as usize);
                        println!(
                            "  │  {}  {:>4} chips  W:{:<2}  {}",
                            &p.player_id[..8],
                            p.chips,
                            p.wins,
                            bar
                        );
                    }
                }
                println!("  └─────────────────────────────────────────┘");
                println!();
            }
        }
        last_hand = state.hand_number;

        // ── Waiting: start next hand ──
        if state.phase == "Waiting" {
            if state.players.len() >= 2 {
                let _ = client.start_hand();
            }
            continue;
        }

        // ── Phase change (only reporter prints) ──
        if state.phase != last_phase && cli.reporter {
            let board = state.community_cards.join(" ");
            match state.phase.as_str() {
                "PreFlop" => println!(
                    "\n  ── PREFLOP ── H#{} pot:{}",
                    state.hand_number + 1,
                    state.pot
                ),
                "Flop" => println!("\n  ── FLOP [ {} ] ── pot:{}", board, state.pot),
                "Turn" => println!("  ── TURN [ {} ] ── pot:{}", board, state.pot),
                "River" => println!("  ── RIVER [ {} ] ── pot:{}", board, state.pot),
                _ => {}
            }
            last_phase = state.phase.clone();
        } else {
            last_phase = state.phase.clone();
        }

        // ── Not my turn ──
        if state.action_on != my_key {
            continue;
        }

        // ── My turn ──
        let my_cards = client.get_my_cards().unwrap_or_default();
        let action = bot.decide(&state, &my_cards);

        let (name_str, result) = match action {
            Action::Fold => ("FOLD", client.fold()),
            Action::Check => ("CHECK", client.check()),
            Action::Call => ("CALL", client.call_bet()),
            Action::RaiseTo(a) => ("RAISE", client.raise_to(a)),
        };

        if result.is_ok() {
            println!(
                "    {:<8} {:<5} {}  pot:{}",
                tag,
                name_str,
                my_cards.join(" "),
                state.pot,
            );
        }
    }
}
