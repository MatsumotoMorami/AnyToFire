use anytofire::score::{self, Store};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args().nth(1).ok_or("用法: score_probe <含 IIDXScore.ini 的目录>")?;
    let config = score::config(Path::new(&root))?;
    println!("数据库: {}", config.database.display());
    println!("作用域: {}", config.scope);
    for (source, charts) in &config.migrations { println!("迁移候选: {source} -> {} 张谱面", charts.len()); }
    let _ = Store::open(config)?;
    println!("兼容数据库打开、建表和幂等迁移成功");
    Ok(())
}
