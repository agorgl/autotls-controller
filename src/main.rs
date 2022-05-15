use std::{collections::BTreeMap, sync::Arc};

use anyhow::Result;
use futures::StreamExt;
use k8s_openapi::api::networking::v1::{Ingress, IngressSpec, IngressTLS};
use kube::{
    api::{ListParams, ObjectMeta, Patch, PatchParams},
    runtime::controller::{Action, Context, Controller},
    Api, Client, ResourceExt,
};
use thiserror::Error;
use tokio::time::Duration;
use tracing::*;

#[derive(Debug, Error)]
enum Error {
    #[error("Failed to patch Ingress: {0}")]
    IngressPatchFailed(#[source] kube::Error),
    #[error("MissingObjectKey: {0}")]
    MissingObjectKey(&'static str),
    #[error(transparent)]
    Unexpected(#[from] anyhow::Error),
}

fn patch_domain(generator: Arc<Ingress>, domain: &str) -> Result<Option<Ingress>> {
    use anyhow::Context;

    let spec = generator
        .spec
        .as_ref()
        .context("spec missing from ingress")?;

    if !match &spec.rules {
        Some(r) if r.len() > 0 => true,
        _ => false,
    } {
        warn!("Ingress has no rules, skipping");
        return Ok(None);
    }

    let mut patched = false;
    let rules = spec.rules.as_ref().map(|r| {
        r.iter()
            .map(|ir| {
                let mut ir = ir.clone();
                if let Some(host) = & mut ir.host {
                    if !host.contains(".") {
                        *host = format!("{host}.{domain}");
                        patched = true;
                    }
                }
                ir
            })
            .collect::<Vec<_>>()
    });

    if !patched {
        return Ok(None);
    }

    let ingress = Ingress {
        metadata: ObjectMeta {
            name: generator.metadata.name.clone(),
            ..ObjectMeta::default()
        },
        spec: Some(IngressSpec {
            rules,
            ..Default::default()
        }),
        ..Default::default()
    };
    Ok(Some(ingress))
}

fn patch_tls(generator: Arc<Ingress>, issuer: &str) -> Result<Option<Ingress>> {
    use anyhow::Context;

    let name = generator
        .metadata
        .name
        .as_ref()
        .context("name missing from ingress metadata")?;
    let spec = generator
        .spec
        .as_ref()
        .context("spec missing from ingress")?;

    if spec.tls.is_some() {
        info!("Ingress {name} already specifies TLS, skipping");
        return Ok(None);
    }

    let hosts = spec
        .rules
        .as_ref()
        .context("rules missing from spec")?
        .iter()
        .filter_map(|r| r.host.as_ref().map(|s| s.clone()))
        .collect::<Vec<_>>();

    let mut annotations = BTreeMap::<String, String>::new();
    annotations.insert(
        "ingress.kubernetes.io/ssl-redirect".to_owned(),
        "true".to_owned(),
    );
    if issuer == "auto" {
        annotations.insert("kubernetes.io/tls-acme".to_owned(), "true".to_owned());
    } else {
        annotations.insert("cert-manager.io/cluster-issuer".to_owned(), issuer.to_owned());
    }

    let ingress = Ingress {
        metadata: ObjectMeta {
            name: generator.metadata.name.clone(),
            annotations: Some(annotations),
            ..ObjectMeta::default()
        },
        spec: Some(IngressSpec {
            tls: Some(vec![IngressTLS {
                hosts: Some(hosts),
                secret_name: Some(format!("{name}-tls")),
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    Ok(Some(ingress))
}

/// Controller triggers this whenever our main object or our children changed
async fn reconcile(generator: Arc<Ingress>, ctx: Context<Data>) -> Result<Action, Error> {
    let client = ctx.get_ref().client.clone();

    let name = generator
        .metadata
        .name
        .as_ref()
        .ok_or(Error::MissingObjectKey(".metadata.name"))?;
    let namespace = generator
        .metadata
        .namespace
        .as_ref()
        .ok_or(Error::MissingObjectKey(".metadata.namespace"))?;
    trace!("Reconciling ingress {name} on namespace {namespace}");

    let api = Api::<Ingress>::namespaced(client.clone(), namespace);

    if let Some(domain) = generator.annotations().get("autotls/domain") {
        let ing = patch_domain(generator.clone(), &domain)?;
        if let Some(ing) = ing {
            info!("Patching domain for ingress {name}");
            api.patch(
                name,
                &PatchParams::apply("autotls-controller/domain-patcher").force(),
                &Patch::Apply(&ing),
            )
            .await
            .map_err(Error::IngressPatchFailed)?;
        }
    }

    if let Some(issuer) = generator.annotations().get("autotls/issuer") {
        let ing = patch_tls(generator.clone(), &issuer)?;
        if let Some(ing) = ing {
            info!("Patching tls for ingress {name}");
            api.patch(
                name,
                &PatchParams::apply("autotls-controller/tls-patcher"),
                &Patch::Apply(&ing),
            )
            .await
            .map_err(Error::IngressPatchFailed)?;
        }
    }

    Ok(Action::requeue(Duration::from_secs(300)))
}

/// The controller triggers this on reconcile errors
fn error_policy(error: &Error, _ctx: Context<Data>) -> Action {
    error!("{error}");
    Action::requeue(Duration::from_secs(1))
}

// Data we want access to in error/reconcile calls
struct Data {
    client: Client,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    info!("Starting autotls controller");

    let client = Client::try_default().await?;
    let ingresses: Api<Ingress> = Api::all(client.clone());

    let context = Context::new(Data { client });
    Controller::new(ingresses, ListParams::default())
        .run(reconcile, error_policy, context)
        .for_each(|res| async move {
            match res {
                Ok((o, _)) => info!("Reconciled ingress {}", o.name),
                Err(e) => warn!("Reconcile failed: {e}"),
            }
        })
        .await;
    Ok(())
}
