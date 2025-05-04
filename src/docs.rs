// #![allow(dead_code)]
use google_docs1::{
    api::{
        CreateParagraphBulletsRequest, Dimension, Document, InsertTextRequest, ParagraphStyle,
        Range, Request, UpdateParagraphStyleRequest,
    },
    common::Client,
    hyper_rustls, hyper_util,
    yup_oauth2::{self, InstalledFlowAuthenticator, InstalledFlowReturnMethod},
    Docs, FieldMask,
};
use google_drive3::{api::File as DriveFile, DriveHub};
use snafu::Snafu;
use std::path::Path;

use crate::activities;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("OAuth error: {}", source))]
    OAuth { source: std::io::Error },

    #[snafu(display("Credentials file not found at: {}", path))]
    CredentialsNotFound { path: String },

    #[snafu(display("Client creation failed: {}", reason))]
    ClientError { reason: String },

    #[snafu(display("Google Docs API error: {}", source))]
    DocsApi { source: google_docs1::Error },
    #[snafu(display("Google Drive API error: {}", source))]
    DriveApi { source: google_drive3::Error },
}

pub type Result<T> = std::result::Result<T, Error>;
type Connector = hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;
type Authenticator = yup_oauth2::authenticator::Authenticator<Connector>;

pub async fn create_authenticator(credentials_path: &str) -> Result<Authenticator> {
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

fn create_client() -> Result<Client<Connector>> {
    hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map(|c| c.https_only().enable_http1().build())
        .map(|connector| {
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(connector)
        })
        .map_err(|err| Error::ClientError {
            reason: err.to_string(),
        })
}

pub async fn create_clients(
    credentials_path: &str,
) -> Result<(Docs<Connector>, DriveHub<Connector>)> {
    let auth = create_authenticator(credentials_path).await?;
    let client = create_client()?;
    let docs = Docs::new(client.clone(), auth.clone());
    let drive = DriveHub::new(client, auth);

    Ok((docs, drive))
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

pub async fn read_template(docs_client: &Docs<Connector>, document_id: &str) -> Result<Document> {
    let (_resp, document) = docs_client
        .documents()
        .get(document_id)
        .doit()
        .await
        .map_err(|e| Error::DocsApi { source: e })?;

    Ok(document)
}

pub async fn copy_template(
    drive: &DriveHub<Connector>,
    template_id: &str,
    title: &str,
) -> Result<String> {
    let mut file: DriveFile = DriveFile::default();
    file.name = Some(title.to_string());

    let result = drive
        .files()
        .copy(file, template_id)
        .doit()
        .await
        .map_err(|e| Error::DriveApi { source: e })?;
    Ok(result.1.id.unwrap())
}

pub async fn fill_placeholders(
    docs_client: &Docs<Connector>,
    document_id: &str,
    activities: &[activities::Activity],
) -> Result<()> {
    let doc = read_template(docs_client, document_id).await?;

    let content = doc.body.unwrap().content.unwrap();

    // extract it to separate functions
    let template_paragraph = content
        .iter()
        .find(|element| {
            element.paragraph.as_ref().is_some_and(|paragraph| {
                paragraph.elements.iter().any(|el| {
                    el.iter().any(|paragraph_element| {
                        paragraph_element.text_run.as_ref().is_some_and(|t| {
                            t.content
                                .as_ref()
                                .is_some_and(|content| content.contains("{repository_name}"))
                        })
                    })
                })
            })
        })
        // TODO: use anyhow, and go outside of GoogleErrors
        .ok_or_else(|| Error::ClientError {
            reason: "Template paragraph with {{repository_name}} not found".to_string(),
        })?;

    println!(
        "{:?}",
        template_paragraph
            .paragraph
            .as_ref()
            .expect("Should not be empty")
    );

    let start_index = template_paragraph.start_index.unwrap();
    let end_index = template_paragraph.end_index.unwrap();

    let mut requests = Vec::new();

    let mut cursor = end_index;

    for activity in activities.iter().filter(|a| !a.activities.is_empty()) {
        let text = template_paragraph
            .paragraph
            .as_ref()
            .expect("should be not empty paragraph")
            .elements
            .iter()
            .flat_map(|vec| vec.iter())
            .filter_map(|paragraph| paragraph.text_run.as_ref().and_then(|t| t.content.clone()))
            .collect::<String>()
            .replace("{repository_name}", &activity.repository);

        requests.push(Request {
            insert_text: Some(InsertTextRequest {
                text: Some(text.clone()),
                location: Some(google_docs1::api::Location {
                    index: Some(cursor),
                    segment_id: None,
                }),
                end_of_segment_location: None,
            }),
            ..Default::default()
        });

        let repo_line_end = cursor + text.len() as i32;
        cursor = repo_line_end;

        let end_index = repo_line_end;
        let start_index = repo_line_end - text.len() as i32;

        requests.push(Request {
            create_paragraph_bullets: Some(CreateParagraphBulletsRequest {
                range: Some(Range {
                    start_index: Some(start_index),
                    end_index: Some(end_index),
                    segment_id: None,
                }),
                bullet_preset: Some("NUMBERED_DECIMAL_ALPHA_ROMAN".to_string()),
            }),
            ..Default::default()
        });

        requests.push(Request {
            update_paragraph_style: Some(UpdateParagraphStyleRequest {
                range: Some(Range {
                    start_index: Some(start_index),
                    end_index: Some(end_index),
                    segment_id: None,
                }),
                paragraph_style: Some(ParagraphStyle {
                    indent_start: Some(Dimension {
                        magnitude: Some(36.0), // level 1 indent
                        unit: Some("PT".to_string()),
                    }),
                    ..Default::default()
                }),
                fields: Some(FieldMask::new(&["indent_start"])),
            }),
            ..Default::default()
        });

        for entry in &activity.activities {
            if entry.to_string().is_empty() {
                continue;
            }

            let commit_text = format!("{}", entry);

            requests.push(Request {
                insert_text: Some(InsertTextRequest {
                    text: Some(commit_text.clone()),
                    location: Some(google_docs1::api::Location {
                        index: Some(cursor),
                        segment_id: None,
                    }),
                    end_of_segment_location: None,
                }),
                ..Default::default()
            });

            let commit_line_end = cursor + commit_text.len() as i32;
            cursor = commit_line_end;

            requests.push(Request {
                create_paragraph_bullets: Some(CreateParagraphBulletsRequest {
                    range: Some(Range {
                        start_index: Some(commit_line_end - commit_text.len() as i32),
                        end_index: Some(commit_line_end),
                        segment_id: None,
                    }),
                    bullet_preset: Some("NUMBERED_DECIMAL_ALPHA_ROMAN".to_string()),
                }),
                ..Default::default()
            });

            requests.push(Request {
                update_paragraph_style: Some(UpdateParagraphStyleRequest {
                    range: Some(Range {
                        start_index: Some(commit_line_end - commit_text.len() as i32),
                        end_index: Some(commit_line_end),
                        segment_id: None,
                    }),
                    paragraph_style: Some(ParagraphStyle {
                        indent_start: Some(Dimension {
                            magnitude: Some(72.0),
                            unit: Some("PT".to_string()),
                        }),
                        ..Default::default()
                    }),
                    fields: Some(FieldMask::new(&["indent_start"])),
                }),
                ..Default::default()
            });
        }
    }
    // Delete the template paragraph
    requests.push(Request {
        delete_content_range: Some(google_docs1::api::DeleteContentRangeRequest {
            range: Some(google_docs1::api::Range {
                segment_id: None,
                start_index: Some(start_index),
                end_index: Some(end_index),
            }),
        }),
        ..Default::default()
    });

    docs_client
        .documents()
        .batch_update(
            google_docs1::api::BatchUpdateDocumentRequest {
                // FIXME: there is a problem with right split,
                // https://docs.google.com/document/d/1kBtF8xmB-qn4UtcoRQyRLgO0c70P2QLp-hldRmP_b5o/edit?tab=t.0
                // generated doc
                // λ cargo run -- --month 4 --user jborkowski --gh-token xxx
                requests: Some(requests),
                ..Default::default()
            },
            document_id,
        )
        .doit()
        .await
        .map_err(|e| Error::DocsApi { source: e })?;

    Ok(())
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

        let (docs_client, _) = match create_clients(credentials_path).await {
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
