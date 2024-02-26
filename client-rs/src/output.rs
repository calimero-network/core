use std::thread;
use std::time::Duration;
use rand::Rng;

use color_eyre::owo_colors::OwoColorize;
use prettytable::{Table, Row, Cell};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::app::{App, InstalledApp};

pub fn print_table_apps(asset: &String, header: &Vec<[&str; 2]>, table_rows: Vec<App>) {
    println!("\nListing {}...\n", asset.yellow());
    let mut table = Table::new();

    let header_row_cells: Vec<Cell> = header[0].iter().map(|&s| Cell::new(s)).collect();
    table.add_row(Row::new(header_row_cells));

    for row_data in table_rows {
        let id_str = row_data.id.to_string();
        let row_cells: Vec<Cell> = vec![Cell::new(&id_str), Cell::new(&row_data.description)];
        table.add_row(Row::new(row_cells));
    }

    table.printstd();
}

pub fn print_table_installed_apps(asset: &String, header: &Vec<[&str; 2]>, table_rows: Vec<InstalledApp>) {
    println!("\nListing {}...\n", asset.yellow());
    let mut table = Table::new();

    let header_row_cells: Vec<Cell> = header[0].iter().map(|&s| Cell::new(s)).collect();
    table.add_row(Row::new(header_row_cells));

    for row_data in table_rows {
        let id_str = row_data.id.to_string();
        let app_id_str = row_data.app_id.to_string();
        let row_cells: Vec<Cell> = vec![Cell::new(&id_str), Cell::new(&app_id_str)];
        table.add_row(Row::new(row_cells));
    }

    table.printstd();
}

pub fn single_progressbar() {
    let pb = ProgressBar::new(10);

    let sty = ProgressStyle::with_template(
        "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
    )
    .unwrap()
    .progress_chars("##-");

    pb.set_style(sty);
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(200));
        pb.inc(1);
    }
    pb.finish_with_message("Done");
}

pub fn multi_progressbar() {
    let m = MultiProgress::new();
    let sty = ProgressStyle::with_template(
        "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
    )
    .unwrap()
    .progress_chars("##-");

    let n = 200;
    let pb = m.add(ProgressBar::new(n));
    pb.set_style(sty.clone());
    pb.set_message("joining network");
    let pb2 = m.add(ProgressBar::new(n));
    pb2.set_style(sty.clone());
    pb2.set_message("connecting to bootstrap node");

    let pb3 = m.insert_after(&pb2, ProgressBar::new(1024));
    pb3.set_style(sty);

    let mut threads = vec![];

    let m_clone = m.clone();
    let h3 = thread::spawn(move || {
        for i in 0..1024 {
            thread::sleep(Duration::from_millis(2));
            pb3.set_message(format!("job #{}", i + 1));
            pb3.inc(1);
        }
        m_clone.println("joined the network!").unwrap();
        pb3.finish();
    });

    for i in 0..n {
        thread::sleep(Duration::from_millis(1));
        if i == n / 3 {
            thread::sleep(Duration::from_millis(50));
        }
        pb.inc(1);
        let pb2 = pb2.clone();
        threads.push(thread::spawn(move || {
            thread::sleep(
                rand::thread_rng().gen_range(Duration::from_secs(1)..Duration::from_secs(2)),
            );
            pb2.inc(1);
        }));
    }
    pb.finish_with_message("all jobs started");

    for thread in threads {
        let _ = thread.join();
    }
    let _ = h3.join();
    pb2.finish();
    m.clear().unwrap();
}
