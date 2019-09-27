#![cfg(feature = "tokio")]

use futures::{
    future::{FusedFuture, FutureExt},
    stream::StreamExt,
};
use std::io;
use tokio_net::signal::ctrl_c;

pub fn default_shutdown_signal() -> io::Result<impl FusedFuture<Output = ()> + Unpin> {
    Ok(ctrl_c()?.into_future().map(|_| ()))
}
