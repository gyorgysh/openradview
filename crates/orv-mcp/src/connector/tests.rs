// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{broker::RequestBroker, http::ServerStatus};
async fn ready(broker: &RequestBroker) {
    for _ in 0..500 {
        match broker.status() {
            ServerStatus::Listening { .. } => return,
            ServerStatus::Starting => tokio::time::sleep(Duration::from_millis(5)).await,
            other => panic!("broker failed: {other:?}"),
        }
    }
    panic!("broker did not start");
}
fn request(id: Option<&str>) -> CallToolRequestParams {
    let mut args = json!({"recipient":"Synthetic discovery test"});
    if let Some(id) = id {
        args["instance_id"] = id.into();
    }
    CallToolRequestParams::new("request_access").with_arguments(args.as_object().unwrap().clone())
}
#[tokio::test]
async fn automatic_discovery_selects_one_requires_choice_for_two_and_keeps_pending_bound() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Registry::at(dir.path().join("instances"));
    let mut connector = Connector::new(None).unwrap();
    connector.discovery = Discovery::Automatic(registry.clone());
    assert!(connector.select(None).is_err());
    let first = RequestBroker::start_inner(0, Arc::new(|| {}), Some(registry.clone())).unwrap();
    ready(&first).await;
    let identity = connector.list().unwrap().pop().unwrap();
    assert_eq!(connector.select(None).unwrap().instance.unwrap(), identity);
    let result = connector.request(request(None)).await.unwrap();
    assert_eq!(result.structured_content.unwrap()["status"], "pending");
    let review = first.take_request().unwrap();
    let second = RequestBroker::start_inner(0, Arc::new(|| {}), Some(registry.clone())).unwrap();
    ready(&second).await;
    assert!(connector.select(None).is_err());
    let other = connector
        .list()
        .unwrap()
        .into_iter()
        .find(|i| i.instance_id != identity.instance_id)
        .unwrap();
    assert!(
        connector
            .request(request(Some(&other.instance_id)))
            .await
            .is_err()
    );
    assert!(second.take_request().is_none());
    first.deny(review.id);
    assert!(
        connector
            .request(request(Some(&identity.instance_id)))
            .await
            .unwrap_err()
            .contains("denied")
    );
    assert!(second.take_request().is_none());
    drop(first);
    for _ in 0..500 {
        if registry.list().unwrap().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let mut fresh = Connector::new(None).unwrap();
    fresh.discovery = Discovery::Automatic(registry);
    assert_eq!(fresh.select(None).unwrap().instance.unwrap(), other);
}
#[tokio::test]
async fn stale_record_cannot_post_to_reused_port_with_another_identity() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Registry::at(dir.path().join("instances"));
    let broker = RequestBroker::start_on(0, Arc::new(|| {})).unwrap();
    ready(&broker).await;
    let ServerStatus::Listening { port } = broker.status() else {
        unreachable!()
    };
    // Deliberately publish the live port with a different instance identity.
    let _lease = registry.publish(Instance::new(port).unwrap()).unwrap();
    let mut connector = Connector::new(None).unwrap();
    connector.discovery = Discovery::Automatic(registry);
    assert!(
        connector
            .request(request(None))
            .await
            .unwrap_err()
            .contains("stopped or changed")
    );
    assert!(broker.take_request().is_none());
    assert!(!connector.state.lock().await.requested);
}
