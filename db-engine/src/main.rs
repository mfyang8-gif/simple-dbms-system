use db_engine::database::Database;
use db_engine::table::Column;

fn main() -> std::io::Result<()> {
    let mut db = Database::open()?;
    println!("=== Mini DBMS Engine Ready ===");

    // 建表
    let mut table = db.create_table("users", vec![
        Column::int("id").not_null().auto_increment(),
        Column::varchar("name", 50).not_null(),
        Column::int("age").check("age >= 0"),
    ])?;
    println!("Created table: {}", table);

    // 插入
    let row = table.new_row()
        .put_int(0)?
        .put_varchar("Alice")?
        .put_int(25)?
        .build()?;
    let row_id = db.insert(&mut table, row)?;
    println!("Inserted: {}", row_id);

    // 关闭
    db.shutdown()?;
    println!("Database closed.");
    Ok(())
}

