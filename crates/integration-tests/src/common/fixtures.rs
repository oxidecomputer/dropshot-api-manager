// Copyright 2025 Oxide Computer Company

//! Test fixtures for common API scenarios in dropshot-api-manager tests.

use chrono::{DateTime, Utc};
use dropshot::{
    HttpError, HttpResponseOk, Path, Query, RequestContext, TypedBody,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A minimal API with just a health check endpoint.
#[dropshot::api_description]
pub trait HealthApi {
    type Context;

    /// Check if the service is healthy.
    #[endpoint { method = GET, path = "/health" }]
    async fn health_check(
        rqctx: RequestContext<Self::Context>,
    ) -> Result<HttpResponseOk<HealthStatus>, HttpError>;
}

/// Health status response.
#[derive(JsonSchema, Serialize)]
pub struct HealthStatus {
    pub status: String,
    pub timestamp: DateTime<Utc>,
}

/// A simple counter API for testing state changes.
#[dropshot::api_description]
pub trait CounterApi {
    type Context;

    /// Get the current counter value.
    #[endpoint { method = GET, path = "/counter" }]
    async fn get_counter(
        rqctx: RequestContext<Self::Context>,
    ) -> Result<HttpResponseOk<CounterValue>, HttpError>;

    /// Set the counter to a specific value.
    #[endpoint { method = PUT, path = "/counter" }]
    async fn set_counter(
        rqctx: RequestContext<Self::Context>,
        body: dropshot::TypedBody<SetCounterRequest>,
    ) -> Result<HttpResponseOk<CounterValue>, HttpError>;

    /// Increment the counter by 1.
    #[endpoint { method = POST, path = "/counter/increment" }]
    async fn increment_counter(
        rqctx: RequestContext<Self::Context>,
    ) -> Result<HttpResponseOk<CounterValue>, HttpError>;

    /// Reset the counter to zero.
    #[endpoint { method = DELETE, path = "/counter" }]
    async fn reset_counter(
        rqctx: RequestContext<Self::Context>,
    ) -> Result<HttpResponseOk<CounterValue>, HttpError>;
}

/// Counter value response.
#[derive(JsonSchema, Serialize)]
pub struct CounterValue {
    pub value: u32,
}

/// Request to set the counter value.
#[derive(JsonSchema, Deserialize)]
pub struct SetCounterRequest {
    pub value: u32,
}

/// A user management API for testing schema evolution.
#[dropshot::api_description]
pub trait UserApi {
    type Context;

    /// List all users.
    #[endpoint { method = GET, path = "/users" }]
    async fn list_users(
        rqctx: RequestContext<Self::Context>,
        query: Query<ListUsersQuery>,
    ) -> Result<HttpResponseOk<UserList>, HttpError>;

    /// Get a specific user by ID.
    #[endpoint { method = GET, path = "/users/{id}" }]
    async fn get_user(
        rqctx: RequestContext<Self::Context>,
        path: Path<UserIdPath>,
    ) -> Result<HttpResponseOk<User>, HttpError>;

    /// Create a new user.
    #[endpoint { method = POST, path = "/users" }]
    async fn create_user(
        rqctx: RequestContext<Self::Context>,
        body: TypedBody<CreateUserRequest>,
    ) -> Result<HttpResponseOk<User>, HttpError>;

    /// Update an existing user.
    #[endpoint { method = PUT, path = "/users/{id}" }]
    async fn update_user(
        rqctx: RequestContext<Self::Context>,
        path: Path<UserIdPath>,
        body: TypedBody<UpdateUserRequest>,
    ) -> Result<HttpResponseOk<User>, HttpError>;

    /// Delete a user.
    #[endpoint { method = DELETE, path = "/users/{id}" }]
    async fn delete_user(
        rqctx: RequestContext<Self::Context>,
        path: Path<UserIdPath>,
    ) -> Result<HttpResponseOk<()>, HttpError>;
}

/// Path parameter for user ID.
#[derive(JsonSchema, Deserialize)]
pub struct UserIdPath {
    pub id: u32,
}

/// Query parameters for listing users.
#[derive(JsonSchema, Deserialize)]
pub struct ListUsersQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub email_filter: Option<String>,
}

/// User information.
#[derive(JsonSchema, Serialize)]
pub struct User {
    pub id: u32,
    pub name: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// List of users response.
#[derive(JsonSchema, Serialize)]
pub struct UserList {
    pub users: Vec<User>,
    pub total_count: u32,
}

/// Request to create a new user.
#[derive(JsonSchema, Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

/// Request to update an existing user.
#[derive(JsonSchema, Deserialize)]
pub struct UpdateUserRequest {
    pub name: Option<String>,
    pub email: Option<String>,
}
