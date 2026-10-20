use super::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// Posts nowhere: keeps the endpoints it was handed and the titles it was asked to show, and
/// answers each with `reply`.
pub(crate) struct Recording {
    sent: Mutex<Vec<String>>,
    titles: Mutex<Vec<String>>,
    reply: Delivery,
}

impl Recording {
    fn answering(reply: Delivery) -> Self {
        Self { sent: Mutex::default(), titles: Mutex::default(), reply }
    }

    /// The titles of the notices posted, in order.
    pub(crate) fn titles(&self) -> Vec<String> {
        self.titles.lock().unwrap().clone()
    }

    /// One whose every post goes through.
    pub(crate) fn taking() -> Self {
        Self::answering(Delivery::Taken)
    }

    /// The endpoints posted to, in order.
    pub(crate) fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl Courier for Recording {
    fn deliver<'a>(
        &'a self,
        subscription: &'a Subscription,
        notice: &'a Notice,
    ) -> Pin<Box<dyn Future<Output = Result<Delivery, web_push::PushError>> + Send + 'a>> {
        self.sent.lock().unwrap().push(subscription.endpoint.clone());
        self.titles.lock().unwrap().push(notice.title.clone());
        let reply = self.reply;
        Box::pin(async move { Ok(reply) })
    }
}

fn notice() -> Notice {
    let (title, body, session) = ("luxe-dev".into(), "Approve bash".into(), "session-1".into());
    Notice { title, body, session, urgent: true }
}

/// Pairs a device named `name`, subscribed at `endpoint` when there is one, and returns its id.
fn device(book: &DeviceBook, name: &str, endpoint: Option<&str>) -> String {
    let token = book.invite().unwrap();
    let (device, _) = book.pair(&token, name, "alex@github").unwrap();
    let keys = SubscriptionKeys {
        p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
            .into(),
        auth: "BTBZMqHH6r4Tts7J_aSIgg".into(),
    };
    let subscription = endpoint.map(|endpoint| Subscription { endpoint: endpoint.into(), keys });
    book.subscribe(&device.id, subscription).unwrap();
    device.id
}

fn book() -> (tempfile::TempDir, DeviceBook) {
    let dir = tempfile::tempdir().unwrap();
    let book = DeviceBook::at(dir.path().join("remote.json"));
    (dir, book)
}

/// The break this catches: a notice sent to a device with that very session on screen, to one
/// that never opted in, or held back from a device that's only looking at another session.
#[tokio::test]
async fn a_notice_reaches_each_subscribed_device_not_looking_at_it() {
    let (_dir, book) = book();
    let presence = Presence::default();
    let elsewhere = device(&book, "iPhone", Some("https://web.push.apple.com/phone"));
    let looking = device(&book, "iPad", Some("https://web.push.apple.com/ipad"));
    device(&book, "Laptop", None);
    let _phone = presence.arrive(&elsewhere, "session-2");
    let _ipad = presence.arrive(&looking, "session-1");

    let courier = Recording::answering(Delivery::Taken);
    dispatch(&book, &presence, &courier, &notice()).await;
    assert_eq!(courier.sent(), ["https://web.push.apple.com/phone"]);
}

fn kept(book: &DeviceBook, id: &str) -> Option<String> {
    let devices = book.devices();
    let device = devices.iter().find(|device| device.id == id)?;
    Some(device.subscription()?.endpoint.clone())
}

/// The break this catches: a subscription Apple has dropped posted to forever (each one a wasted
/// request, and a 410 in the log), or one forgotten after a post that went through.
#[tokio::test]
async fn a_subscription_is_forgotten_only_when_its_service_says_it_is_gone() {
    let (_dir, book) = book();
    let id = device(&book, "iPhone", Some("https://web.push.apple.com/phone"));
    let presence = Presence::default();

    dispatch(&book, &presence, &Recording::answering(Delivery::Taken), &notice()).await;
    assert_eq!(kept(&book, &id).as_deref(), Some("https://web.push.apple.com/phone"));
    dispatch(&book, &presence, &Recording::answering(Delivery::Gone), &notice()).await;
    assert_eq!(kept(&book, &id), None);
}

/// A named bridge's notice says which computer it's from, after the project; an unnamed one is
/// left as the session wrote it.
///
/// The break this catches: a phone paired with two computers told "luxe-dev" wants it, with no
/// way to know which computer to open, or the body or session changed along with the title.
#[test]
fn a_notice_names_the_computer_it_came_from() {
    let named = on_machine(notice(), Some("desk"));
    assert_eq!(named.title, "luxe-dev \u{00b7} desk");
    assert_eq!((named.body.as_str(), named.session.as_str()), ("Approve bash", "session-1"));
    assert_eq!(on_machine(notice(), None).title, "luxe-dev");
}
