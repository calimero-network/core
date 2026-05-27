use calimero_server_primitives::admin::NetworkStatusResponse;
use comfy_table::{Cell, Color, Table};

use super::Report;

impl Report for NetworkStatusResponse {
    fn report(&self) {
        // Identity table — peer id + every address the swarm currently knows about.
        let mut identity = Table::new();
        let _ = identity.set_header(vec![
            Cell::new("Identity").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = identity.add_row(vec!["Local peer ID", &self.local_peer_id]);
        for (idx, addr) in self.listen_addrs.iter().enumerate() {
            let _ = identity.add_row(vec![&format!("Listen [{idx}]"), addr]);
        }
        if self.listen_addrs.is_empty() {
            let _ = identity.add_row(vec!["Listen", "(none)"]);
        }
        for (idx, addr) in self.external_addrs.iter().enumerate() {
            let _ = identity.add_row(vec![&format!("External [{idx}]"), addr]);
        }
        if self.external_addrs.is_empty() {
            let _ = identity.add_row(vec!["External", "(none)"]);
        }
        println!("{identity}");

        // AutoNAT — reachability + last probe.
        let mut autonat = Table::new();
        let _ = autonat.set_header(vec![
            Cell::new("AutoNAT").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = autonat.add_row(vec!["Reachability", &self.autonat.reachability]);
        let _ = autonat.add_row(vec![
            "Last test addr",
            self.autonat.last_test_addr.as_deref().unwrap_or("(none)"),
        ]);
        let _ = autonat.add_row(vec![
            "Last test result",
            self.autonat.last_test_result.as_deref().unwrap_or("(none)"),
        ]);
        if let Some(ref observed) = self.autonat.last_test_observed_addr {
            let _ = autonat.add_row(vec!["Observed addr", observed]);
        }
        if let Some(ref reason) = self.autonat.last_test_reason {
            let _ = autonat.add_row(vec!["Failure reason", reason]);
        }
        let _ = autonat.add_row(vec![
            "Last test at",
            self.autonat.last_test_at.as_deref().unwrap_or("(none)"),
        ]);
        println!("{autonat}");

        // Relays.
        let mut relays = Table::new();
        let _ = relays.set_header(vec![
            Cell::new("Relay peer").fg(Color::Blue),
            Cell::new("Reservation").fg(Color::Blue),
            Cell::new("Last state change").fg(Color::Blue),
        ]);
        if self.relays.is_empty() {
            let _ = relays.add_row(vec!["(no known relays)", "", ""]);
        } else {
            for r in &self.relays {
                let _ = relays.add_row(vec![
                    &r.peer_id,
                    &r.reservation_status,
                    &r.last_state_change,
                ]);
            }
        }
        println!("{relays}");

        // Rendezvous.
        let mut rdv = Table::new();
        let _ = rdv.set_header(vec![
            Cell::new("Rendezvous peer").fg(Color::Blue),
            Cell::new("Registration").fg(Color::Blue),
            Cell::new("Last state change").fg(Color::Blue),
        ]);
        if self.rendezvous.is_empty() {
            let _ = rdv.add_row(vec!["(no known rendezvous)", "", ""]);
        } else {
            for r in &self.rendezvous {
                let _ = rdv.add_row(vec![
                    &r.peer_id,
                    &r.registration_status,
                    &r.last_state_change,
                ]);
            }
        }
        println!("{rdv}");

        // Direct upgrades (DCUtR).
        let mut dc = Table::new();
        let _ = dc.set_header(vec![
            Cell::new("DCUtR peer").fg(Color::Blue),
            Cell::new("Status").fg(Color::Blue),
            Cell::new("Detail").fg(Color::Blue),
            Cell::new("Last attempt").fg(Color::Blue),
        ]);
        if self.direct_upgrades.is_empty() {
            let _ = dc.add_row(vec!["(no upgrade attempts observed)", "", "", ""]);
        } else {
            for d in &self.direct_upgrades {
                let detail = d
                    .reason
                    .as_deref()
                    .or(d.connection_id.as_deref())
                    .unwrap_or("-");
                let _ = dc.add_row(vec![&d.peer_id, &d.status, detail, &d.last_attempt]);
            }
        }
        println!("{dc}");
    }
}
