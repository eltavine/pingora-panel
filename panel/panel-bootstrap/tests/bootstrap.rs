#![forbid(unsafe_code)]

use panel_bootstrap::{Plan, ADMIN_DATABASE_URL_ENV, SERVICE_SCHEMAS};
use panel_control_runtime::NATS_URL_ENV;
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_postgres::{
    testing::TestDatabase, ServiceDatabase, ServiceDatabaseConfig, SqlIdentifier,
};
use panel_service::Environment;
use std::{collections::HashMap, ffi::OsString};

#[test]
fn every_service_password_is_required() {
    let mut env = Environment::from_lookup(|name| {
        (name == ADMIN_DATABASE_URL_ENV).then(|| OsString::from("postgres://admin@db/panel"))
    });
    let error = Plan::read(&mut env).err().expect("passwords are missing");
    assert!(error
        .message
        .contains("PINGORA_PANEL_IDENTITY_DATABASE_PASSWORD"));
}

#[tokio::test]
async fn provisioning_is_repeatable_and_admits_each_service() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let mut values = HashMap::from([
        (
            ADMIN_DATABASE_URL_ENV.to_owned(),
            database.admin_url().to_owned(),
        ),
        (
            NATS_URL_ENV.to_owned(),
            std::env::var(TEST_NATS_URL_ENV).unwrap(),
        ),
    ]);
    for (schema, password_env) in SERVICE_SCHEMAS {
        values.insert((*password_env).to_owned(), format!("{schema}-secret"));
    }
    let lookup = values.clone();
    let prefix = database.role_name("").as_str().to_owned();
    let plan = Plan::read(&mut Environment::from_lookup(move |name| {
        lookup.get(name).map(OsString::from)
    }))
    .unwrap()
    .with_role_prefix(prefix.clone())
    .with_jetstream_settings((*broker.settings).clone());
    for (schema, _) in SERVICE_SCHEMAS {
        database.adopt_role(plan.role_name(schema).unwrap());
    }

    plan.apply().await.unwrap();
    plan.apply().await.unwrap();

    let url = database.admin_url();
    let (server, _) = url.split_once("://").unwrap();
    let host = url.rsplit_once('@').unwrap().1;
    for (schema, _) in SERVICE_SCHEMAS {
        let role = plan.role_name(schema).unwrap();
        let config = ServiceDatabaseConfig::new(
            &format!("{server}://{role}:{schema}-secret@{host}"),
            "bootstrap-test",
            SqlIdentifier::new(*schema).unwrap(),
        )
        .unwrap();
        let service = ServiceDatabase::connect(config).await.unwrap();
        service.migrate(&[]).await.unwrap();
        service.close().await;
    }
    for stream in [
        broker.settings.events_stream(),
        broker.settings.dead_letter_stream(),
    ] {
        broker.context.get_stream(stream).await.unwrap();
    }
    broker
        .context
        .get_key_value(broker.settings.service_bucket())
        .await
        .unwrap();

    broker
        .context
        .delete_key_value(broker.settings.service_bucket())
        .await
        .unwrap();
    database.drop().await;
    broker.drop().await;
}
