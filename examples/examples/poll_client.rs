use pico_args::Arguments;
use polyfuse_examples::fs::FileDesc;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, PollEvented};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = Arguments::from_env();
    let is_nonblocking = args.contains("--nonblock");
    let path: PathBuf = args
        .free_from_str()?
        .ok_or_else(|| anyhow::anyhow!("missing path"))?;

    let content = if is_nonblocking {
        let fd = FileDesc::open(&path, libc::O_RDONLY | libc::O_NONBLOCK).await?;
        let mut evented = PollEvented::new(fd)?;
        let mut buf = String::new();
        evented.read_to_string(&mut buf).await?;
        buf
    } else {
        tokio::fs::read_to_string(&path).await?
    };

    println!("read: {:?}", content);

    Ok(())
}
