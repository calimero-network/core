use actix::{Actor, Context, Handler, Message};

use crate::LazyRecipient;

struct MyActor;

impl Actor for MyActor {
    type Context = Context<Self>;
}

#[derive(Message)]
#[rtype("()")]
struct MyMessage;

impl Handler<MyMessage> for MyActor {
    type Result = ();

    fn handle(&mut self, _msg: MyMessage, _ctx: &mut Context<Self>) -> Self::Result {
        ()
    }
}

#[actix::test]
#[should_panic = "attempted illegal use of uninitialized `Recipient<calimero_utils_actix::recipient::recipient_tests::MyMessage>`"]
async fn uninitialized() {
    let recipient = LazyRecipient::new_uninit();

    recipient.send(MyMessage).await.unwrap();
}

#[actix::test]
async fn initialized() {
    let mut recipient = LazyRecipient::new_uninit();

    recipient.init(MyActor.start());

    recipient.send(MyMessage).await.unwrap();
}
