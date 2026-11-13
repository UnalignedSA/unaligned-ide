// Modified by the Unaligned IDE contributors, 2026: see NOTICE at the repository root.
mod llm_token;
mod websocket;

use std::sync::Arc;

use anyhow::{Result, anyhow};
pub use cloud_api_types::*;
use futures::AsyncReadExt as _;
use http_client::http::request;
use http_client::{
    AsyncBody, HttpClientWithUrl, HttpRequestExt, Json, Method, Request, Response, StatusCode,
};
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use thiserror::Error;

pub use llm_token::LlmApiToken;

struct Credentials {
    user_id: u32,
    access_token: String,
}

#[derive(Clone, Copy)]
enum Authentication {
    Credentials,
    Session,
}

#[derive(Debug, Error)]
pub enum ClientApiError {
    /// 401 — credentials are invalid or expired.
    #[error("Unauthorized")]
    Unauthorized,
    /// No credentials have been set on the client.
    #[error("not signed in")]
    NotSignedIn,
    /// Connection-level failure: DNS, TCP, TLS, timeout, etc.
    /// The HTTP request never received a response.
    #[error("connection to {host} failed")]
    ConnectionFailed {
        host: String,
        #[source]
        source: anyhow::Error,
    },
    /// Server returned a non-success HTTP status (other than 401).
    #[error("{host} returned {status}")]
    ServerError {
        host: String,
        status: StatusCode,
        body: String,
    },
    /// Failed to read or parse the response body after a successful HTTP status.
    #[error("invalid response")]
    InvalidResponse(#[source] anyhow::Error),
    /// Failed to build the HTTP request (URL construction, serialization, etc.).
    /// This typically indicates a programming error.
    #[error("failed to build request")]
    RequestBuildFailed(#[source] anyhow::Error),
}

pub struct CloudApiClient {
    credentials: RwLock<Option<Credentials>>,
    http_client: Arc<HttpClientWithUrl>,
    authentication: Authentication,
}

impl CloudApiClient {
    pub fn new(http_client: Arc<HttpClientWithUrl>) -> Self {
        Self {
            credentials: RwLock::new(None),
            http_client,
            authentication: Authentication::Credentials,
        }
    }

    pub fn new_with_session_authentication(http_client: Arc<HttpClientWithUrl>) -> Self {
        Self {
            credentials: RwLock::new(None),
            http_client,
            authentication: Authentication::Session,
        }
    }

    pub fn has_credentials(&self) -> bool {
        self.credentials.read().is_some()
    }

    pub fn set_credentials(&self, user_id: u32, access_token: String) {
        *self.credentials.write() = Some(Credentials {
            user_id,
            access_token,
        });
    }

    pub fn clear_credentials(&self) {
        *self.credentials.write() = None;
    }

    pub fn cloud_host(&self) -> String {
        self.http_client
            .build_zed_cloud_url("/")
            .ok()
            .and_then(|url| url.host_str().map(String::from))
            .unwrap_or_else(|| "cloud.zed.dev".into())
    }

    pub async fn get_authenticated_user(
        &self,
        system_id: Option<String>,
    ) -> Result<GetAuthenticatedUserResponse, ClientApiError> {
        let request_builder = Request::builder()
            .method(Method::GET)
            .uri(
                self.http_client
                    .build_zed_cloud_url("/client/users/me")
                    .map_err(ClientApiError::RequestBuildFailed)?
                    .as_ref(),
            )
            .when_some(system_id, |builder, system_id| {
                builder.header(IDE_SYSTEM_ID_HEADER_NAME, system_id)
            });

        self.send_authenticated_json_request(request_builder, AsyncBody::default())
            .await
    }

    async fn create_llm_token(
        &self,
        system_id: Option<String>,
        organization_id: OrganizationId,
    ) -> Result<CreateLlmTokenResponse, ClientApiError> {
        let request_builder = Request::builder()
            .method(Method::POST)
            .uri(
                self.http_client
                    .build_zed_cloud_url("/client/llm_tokens")
                    .map_err(ClientApiError::RequestBuildFailed)?
                    .as_ref(),
            )
            .when_some(system_id, |builder, system_id| {
                builder.header(IDE_SYSTEM_ID_HEADER_NAME, system_id)
            });

        self.send_authenticated_json_request(
            request_builder,
            Json(CreateLlmTokenBody { organization_id }),
        )
        .await
    }

    pub async fn update_system_settings(
        &self,
        system_id: String,
        body: UpdateSystemSettingsBody,
    ) -> Result<SystemSettings, ClientApiError> {
        let request_builder = Request::builder()
            .method(Method::PATCH)
            .uri(
                self.http_client
                    .build_zed_cloud_url("/client/system_settings")
                    .map_err(ClientApiError::RequestBuildFailed)?
                    .as_ref(),
            )
            .header(IDE_SYSTEM_ID_HEADER_NAME, system_id);

        self.send_authenticated_json_request(request_builder, Json(body))
            .await
    }

    pub async fn send_authenticated_json_request<T: DeserializeOwned>(
        &self,
        request_builder: request::Builder,
        body: impl Into<AsyncBody>,
    ) -> Result<T, ClientApiError> {
        let mut response = self
            .send_authenticated_request(request_builder, body)
            .await?;
        Self::read_response_json(&mut response).await
    }

    async fn send_authenticated_request(
        &self,
        request_builder: request::Builder,
        body: impl Into<AsyncBody>,
    ) -> Result<Response<AsyncBody>, ClientApiError> {
        let request = if matches!(self.authentication, Authentication::Session) {
            build_request(request_builder, body, None)
                .map_err(ClientApiError::RequestBuildFailed)?
        } else {
            let credentials = self.credentials.read();
            let credentials = credentials.as_ref().ok_or(ClientApiError::NotSignedIn)?;
            build_request(request_builder, body, Some(credentials))
                .map_err(ClientApiError::RequestBuildFailed)?
        };

        let host = self.cloud_host();
        let mut response = self.http_client.send(request).await.map_err(|source| {
            ClientApiError::ConnectionFailed {
                host: host.clone(),
                source,
            }
        })?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        if status == StatusCode::UNAUTHORIZED {
            return Err(ClientApiError::Unauthorized);
        }

        let body = match Self::read_response_body(&mut response).await {
            Ok(body) => body,
            Err(error) => format!("failed to read response body: {error}"),
        };
        Err(ClientApiError::ServerError { host, status, body })
    }

    async fn read_response_json<T: DeserializeOwned>(
        response: &mut Response<AsyncBody>,
    ) -> Result<T, ClientApiError> {
        let body = Self::read_response_body(response).await?;
        serde_json::from_str(&body).map_err(|error| ClientApiError::InvalidResponse(error.into()))
    }

    async fn read_response_body(
        response: &mut Response<AsyncBody>,
    ) -> Result<String, ClientApiError> {
        let mut body = String::new();
        response
            .body_mut()
            .read_to_string(&mut body)
            .await
            .map_err(|error| ClientApiError::InvalidResponse(error.into()))?;
        Ok(body)
    }

    pub async fn validate_credentials(&self, user_id: u32, access_token: &str) -> Result<bool> {
        let request = build_request(
            Request::builder().method(Method::GET).uri(
                self.http_client
                    .build_zed_cloud_url("/client/users/me")?
                    .as_ref(),
            ),
            AsyncBody::default(),
            Some(&Credentials {
                user_id,
                access_token: access_token.into(),
            }),
        )?;

        let mut response = self.http_client.send(request).await?;

        if response.status().is_success() {
            Ok(true)
        } else {
            let mut body = String::new();
            response.body_mut().read_to_string(&mut body).await?;
            if response.status() == StatusCode::UNAUTHORIZED {
                Ok(false)
            } else {
                Err(anyhow!(
                    "Failed to get authenticated user.\nStatus: {:?}\nBody: {body}",
                    response.status()
                ))
            }
        }
    }

    /// Feedback about agent threads went to Zed's cloud together with the
    /// thread's content. This build sends none: the call fails before any
    /// request is built, whatever the server URL is.
    pub async fn submit_agent_feedback(&self, _body: SubmitAgentThreadFeedbackBody) -> Result<()> {
        Err(feedback_unavailable())
    }

    /// See [`Self::submit_agent_feedback`]: fails without a request.
    pub async fn submit_agent_feedback_comments(
        &self,
        _body: SubmitAgentThreadFeedbackCommentsBody,
    ) -> Result<()> {
        Err(feedback_unavailable())
    }

    /// See [`Self::submit_agent_feedback`]: fails without a request.
    pub async fn submit_edit_prediction_feedback(
        &self,
        _body: SubmitEditPredictionFeedbackBody,
    ) -> Result<()> {
        Err(feedback_unavailable())
    }
}

fn feedback_unavailable() -> anyhow::Error {
    anyhow!("Feedback is not sent to Zed's cloud in this build")
}

fn build_request(
    req: request::Builder,
    body: impl Into<AsyncBody>,
    credentials: Option<&Credentials>,
) -> Result<Request<AsyncBody>> {
    Ok(req
        .header("Content-Type", "application/json")
        .when_some(credentials, |request, credentials| {
            request.header(
                "Authorization",
                format!("{} {}", credentials.user_id, credentials.access_token),
            )
        })
        .body(body.into())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_client::BlockedHttpClient;

    /// A client whose every request fails, with Zed's production server as
    /// its base URL: what the app has when nothing overrides the server.
    fn production_like_client() -> CloudApiClient {
        CloudApiClient::new(Arc::new(HttpClientWithUrl::new_url(
            Arc::new(BlockedHttpClient::new()),
            "https://zed.dev",
            None,
        )))
    }

    #[test]
    fn feedback_calls_fail_without_a_request() {
        let client = production_like_client();
        client.set_credentials(1, "token".into());

        let error = futures::executor::block_on(client.submit_agent_feedback(
            SubmitAgentThreadFeedbackBody {
                organization_id: OrganizationId("organization".into()),
                agent: "agent".into(),
                session_id: "session".into(),
                parent_session_id: None,
                rating: "positive".into(),
                thread: serde_json::Value::Null,
            },
        ))
        .unwrap_err();

        // `BlockedHttpClient` answers "disallowed request" to anything that is
        // sent: this message proves nothing was.
        assert_eq!(
            error.to_string(),
            "Feedback is not sent to Zed's cloud in this build"
        );
    }

    #[test]
    fn cloud_requests_fail_before_they_are_sent() {
        // The override enables Zed's services on purpose.
        if std::env::var_os("IDE_SERVER_URL").is_some_and(|value| !value.is_empty()) {
            return;
        }

        let client = production_like_client();
        client.set_credentials(1, "token".into());

        let error = futures::executor::block_on(client.get_authenticated_user(None)).unwrap_err();
        assert!(
            matches!(error, ClientApiError::RequestBuildFailed(_)),
            "{error:?}"
        );

        let error =
            futures::executor::block_on(client.validate_credentials(1, "token")).unwrap_err();
        assert_eq!(
            error.to_string(),
            http_client::IDE_CLOUD_UNAVAILABLE_MESSAGE
        );
    }

    #[test]
    fn build_session_authenticated_request_without_authorization_header() -> Result<()> {
        let request = build_request(
            Request::builder().uri("https://cloud.zed.dev/client/users/me"),
            AsyncBody::default(),
            None,
        )?;

        assert_eq!(
            request
                .headers()
                .get("Content-Type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert!(!request.headers().contains_key("Authorization"));
        Ok(())
    }

    #[test]
    fn build_credentials_authenticated_request_with_authorization_header() -> Result<()> {
        let request = build_request(
            Request::builder().uri("https://cloud.zed.dev/client/users/me"),
            AsyncBody::default(),
            Some(&Credentials {
                user_id: 123,
                access_token: "token".into(),
            }),
        )?;

        assert_eq!(
            request
                .headers()
                .get("Authorization")
                .and_then(|value| value.to_str().ok()),
            Some("123 token")
        );
        Ok(())
    }
}
