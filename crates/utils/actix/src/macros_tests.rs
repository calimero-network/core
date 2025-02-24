use core::array::IntoIter;

use actix::{Actor, Addr, Context, Handler, Message, Response, StreamHandler};
use futures_util::stream::{self, Iter, Repeat, StreamExt, Take, Zip};
use tokio::time::{self, Instant};
use tokio_stream::wrappers::IntervalStream;

use crate::spawn_actor;

struct MyActor {
    total: usize,
    stream1: Box<Take<Repeat<usize>>>,
    current_name: &'static str,
    stream2: Box<Zip<IntervalStream, Iter<IntoIter<&'static str, 26>>>>,
}

impl Actor for MyActor {
    type Context = Context<Self>;

    fn start(mut self) -> Addr<Self> {
        spawn_actor!(self @ MyActor => {
            .stream1,
            .stream2 as Name,
        })
    }
}

impl StreamHandler<usize> for MyActor {
    fn handle(&mut self, item: usize, _: &mut Context<Self>) {
        self.total += item;
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {}
}

struct Name {
    name: &'static str,
}

impl From<(Instant, &'static str)> for Name {
    fn from((_, name): (Instant, &'static str)) -> Self {
        Self { name }
    }
}

impl StreamHandler<Name> for MyActor {
    fn handle(&mut self, item: Name, _: &mut Context<Self>) {
        self.current_name = item.name;
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {}
}

#[derive(Message)]
#[rtype(usize)]
struct GetTotal;

impl Handler<GetTotal> for MyActor {
    type Result = usize;

    fn handle(&mut self, _: GetTotal, _: &mut Context<Self>) -> Self::Result {
        self.total
    }
}

#[derive(Message)]
#[rtype("&'static str")]
struct GetCurrentName;

impl Handler<GetCurrentName> for MyActor {
    type Result = Response<&'static str>;

    fn handle(&mut self, _: GetCurrentName, _: &mut Context<Self>) -> Self::Result {
        Response::reply(self.current_name)
    }
}

#[actix::test]
async fn test() -> eyre::Result<()> {
    let names = stream::iter([
        "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Heidi", "Ivan", "Judy", "Kevin",
        "Larry", "Mallory", "Nancy", "Olivia", "Peggy", "Quentin", "Ralph", "Steve", "Trent",
        "Ursula", "Victor", "Walter", "Xavier", "Yvonne", "Zelda",
    ]);

    let interval = time::interval(time::Duration::from_millis(200));

    let addr = MyActor {
        total: 0,
        stream1: Box::new(stream::repeat(10).take(5)),
        current_name: "",
        stream2: Box::new(IntervalStream::new(interval).zip(names)),
    }
    .start();

    let mut interval = time::interval(time::Duration::from_secs(2));

    let _ignored = interval.tick().await;

    let total = addr.send(GetTotal).await?;

    assert_eq!(50, total);

    let _ignored = interval.tick().await;

    let e = addr.send(GetCurrentName).await?;

    assert_eq!("Judy", e);

    Ok(())
}
