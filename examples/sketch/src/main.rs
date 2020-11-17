use polyfuse::op;
use polyfuse_daemon::Operation;

fn main() -> anyhow::Result<()> {
    async_std::task::block_on(async {
        let mut daemon = polyfuse_daemon::mount("/path/to/mountpoint", &[]).await?;

        let fs = Filesystem {};

        loop {
            let mut req = match daemon.next_request().await {
                Some(req) => req,
                None => break,
            };

            req.process(|op| fs.process(op)).await?;
        }

        Ok(())
    })
}

struct Filesystem {}

impl Filesystem {
    async fn process(&self, op: Operation<'_>) -> Result<(), polyfuse_daemon::Error> {
        match op {
            Operation::Getattr(op) => self.getattr(op).await,
            op => op.default().await,
        }
    }

    async fn getattr<Op>(&self, _op: Op) -> Result<Op::Ok, Op::Error>
    where
        Op: op::Getattr,
    {
        todo!()
    }
}
