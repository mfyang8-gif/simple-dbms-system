use db_engine::database::{Database, DbRegistry};
use db_engine::server::{run_repl, run_server};

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("--repl") => {
            let db = Database::open()?;
            let mut reg = DbRegistry::new(db);
            run_repl(&mut reg);
            reg.shutdown();
            println!("Bye.");
            Ok(())
        }
        Some(addr) => {
            let db = Database::open()?;
            run_server(DbRegistry::new(db), addr)
        }
        None => {
            let db = Database::open()?;
            run_server(DbRegistry::new(db), "127.0.0.1:9000")
        }
    }
}