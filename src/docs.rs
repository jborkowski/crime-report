// #![allow(dead_code)]
use google_docs1::{
    hyper_rustls, hyper_util,
    yup_oauth2::{self, InstalledFlowAuthenticator, InstalledFlowReturnMethod},
    Docs,
};
use snafu::Snafu;
use std::path::Path;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("OAuth error: {}", source))]
    OAuth { source: std::io::Error },

    #[snafu(display("Credentials file not found at: {}", path))]
    CredentialsNotFound { path: String },

    #[snafu(display("Google Docs API error: {}", source))]
    DocsApi { source: google_docs1::Error },
}

type Result<T> = std::result::Result<T, Error>;
type Connector = hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;

async fn create_authenticator(
    credentials_path: &str,
) -> std::result::Result<
    yup_oauth2::authenticator::Authenticator<
        yup_oauth2::hyper_rustls::HttpsConnector<
            google_docs1::hyper_util::client::legacy::connect::HttpConnector,
        >,
    >,
    Error,
> {
    let path = Path::new(credentials_path);
    if !path.exists() {
        return Err(Error::CredentialsNotFound {
            path: credentials_path.to_string(),
        });
    }

    let secret = yup_oauth2::read_application_secret(credentials_path)
        .await
        .map_err(|e| Error::OAuth { source: e })?;

    let auth = InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
        .persist_tokens_to_disk("tokencache.json")
        .build()
        .await
        .map_err(|e| Error::OAuth { source: e })?;

    Ok(auth)
}

pub async fn create_docs_client(credentials_path: &str) -> Result<Docs<Connector>> {
    let auth = create_authenticator(credentials_path).await?;

    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .unwrap()
        .https_only()
        .enable_http1()
        .build();

    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector);

    //.map_err(|e| Error::Reqwest { source: e })?;

    let docs = Docs::new(client, auth);

    Ok(docs)
}

pub async fn create_document(docs_client: &Docs<Connector>, title: &str) -> Result<String> {
    let document = google_docs1::api::Document {
        title: Some(title.to_string()),
        ..Default::default()
    };

    let result = docs_client
        .documents()
        .create(document)
        .doit()
        .await
        .map_err(|e| Error::DocsApi { source: e })?;

    Ok(result.1.document_id.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_document() {
        // This test requires valid credentials.json file
        // Skip the test if the file doesn't exist
        // NOTE: To authorize Google Docs API access, run the test with:
        // `cargo test test_create_document -- --nocapture`
        // This will display the OAuth authorization URL in the console and wait for authentication
        let credentials_path = "credentials.json";
        if !Path::new(credentials_path).exists() {
            println!("Skipping test_create_document: credentials.json not found");
            return;
        }

        let docs_client = match create_docs_client(credentials_path).await {
            Ok(client) => client,
            Err(e) => {
                println!(
                    "Skipping test_create_document: Failed to create docs client: {}",
                    e
                );
                return;
            }
        };

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let title = format!("Test Document {}", timestamp);

        let document_id = match create_document(&docs_client, &title).await {
            Ok(id) => id,
            Err(e) => {
                panic!("Failed to create document: {}", e);
            }
        };

        assert!(!document_id.is_empty(), "Document ID should not be empty");

        println!("Successfully created document with ID: {}", document_id);
    }
}

