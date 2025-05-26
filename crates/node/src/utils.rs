use std::pin::pin;

use futures_util::{Stream, StreamExt};
use rand::Rng;

// todo! consider `T: IntoResult`, so we transpose internally
pub async fn choose_stream<T>(stream: impl Stream<Item = T>, rng: &mut impl Rng) -> Option<T> {
    let mut stream = pin!(stream);

    let mut item = stream.next().await;

    let mut stream = stream.enumerate();

    while let Some((idx, this)) = stream.next().await {
        if rng.gen_range(0..idx + 1) == 0 {
            item = Some(this);
        }
    }

    item
}
