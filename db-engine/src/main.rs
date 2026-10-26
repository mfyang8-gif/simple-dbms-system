use db_engine::database::Database;
use db_engine::server::{run_repl, run_server};

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("--repl") => {
            let mut db = Database::open()?;
            run_repl(&mut db);
            db.shutdown()?;
            println!("Bye.");
            Ok(())
        }
        Some(addr) => {
            let db = Database::open()?;
            run_server(db, addr)
        }
        None => {
            let db = Database::open()?;
            run_server(db, "127.0.0.1:9000")
        }
    }
}