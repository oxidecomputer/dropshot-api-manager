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

/// Versioned health API for testing version evolution.
pub mod versioned_health {
    use super::*;
    use dropshot_api_manager_types::api_versions;

    api_versions!(
        [(3, WITH_METRICS), (2, WITH_DETAILED_STATUS), (1, INITIAL),]
    );

    #[dropshot::api_description]
    pub trait VersionedHealthApi {
        type Context;

        /// Check if the service is healthy (all versions).
        #[endpoint {
            method = GET,
            path = "/health",
            operation_id = "health_check",
            versions = "1.0.0"..
        }]
        async fn health_check(
            rqctx: RequestContext<Self::Context>,
        ) -> Result<HttpResponseOk<HealthStatusV1>, HttpError>;

        /// Get detailed health status (v2+).
        #[endpoint {
            method = GET,
            path = "/health/detailed",
            operation_id = "detailed_health_check",
            versions = "2.0.0"..
        }]
        async fn detailed_health_check(
            rqctx: RequestContext<Self::Context>,
        ) -> Result<HttpResponseOk<DetailedHealthStatus>, HttpError>;

        /// Get service metrics (v3+).
        #[endpoint {
            method = GET,
            path = "/metrics",
            operation_id = "get_metrics",
            versions = "3.0.0"..
        }]
        async fn get_metrics(
            rqctx: RequestContext<Self::Context>,
        ) -> Result<HttpResponseOk<ServiceMetrics>, HttpError>;
    }

    /// Basic health status response (v1).
    #[derive(JsonSchema, Serialize)]
    pub struct HealthStatusV1 {
        pub status: String,
        pub timestamp: DateTime<Utc>,
    }

    /// Detailed health status response (v2+).
    #[derive(JsonSchema, Serialize)]
    pub struct DetailedHealthStatus {
        pub status: String,
        pub timestamp: DateTime<Utc>,
        pub uptime_seconds: u64,
        pub dependencies: Vec<DependencyStatus>,
    }

    /// Dependency status information.
    #[derive(JsonSchema, Serialize)]
    pub struct DependencyStatus {
        pub name: String,
        pub status: String,
        pub response_time_ms: Option<u64>,
    }

    /// Service metrics response (v3+).
    #[derive(JsonSchema, Serialize)]
    pub struct ServiceMetrics {
        pub requests_per_second: f64,
        pub error_rate: f64,
        pub avg_response_time_ms: f64,
        pub active_connections: u32,
    }
}

/// Versioned user API for testing complex schema evolution.
pub mod versioned_user {
    use super::*;
    use dropshot_api_manager_types::api_versions;

    api_versions!([
        (3, WITH_ROLES_AND_PERMISSIONS),
        (2, WITH_PROFILE_DATA),
        (1, INITIAL),
    ]);

    #[dropshot::api_description]
    pub trait VersionedUserApi {
        type Context;

        /// List all users (all versions).
        #[endpoint {
            method = GET,
            path = "/users",
            operation_id = "list_users",
            versions = "1.0.0"..VERSION_WITH_PROFILE_DATA
        }]
        async fn list_users_v1(
            rqctx: RequestContext<Self::Context>,
            query: Query<ListUsersQueryV1>,
        ) -> Result<HttpResponseOk<UserListV1>, HttpError>;

        /// List all users with profile data (v2+).
        #[endpoint {
            method = GET,
            path = "/users",
            operation_id = "list_users",
            versions = "2.0.0"..
        }]
        async fn list_users_v2(
            rqctx: RequestContext<Self::Context>,
            query: Query<ListUsersQueryV2>,
        ) -> Result<HttpResponseOk<UserListV2>, HttpError>;

        /// Get a specific user by ID (all versions).
        #[endpoint {
            method = GET,
            path = "/users/{id}",
            operation_id = "get_user",
            versions = "1.0.0"..VERSION_WITH_PROFILE_DATA
        }]
        async fn get_user_v1(
            rqctx: RequestContext<Self::Context>,
            path: Path<UserIdPath>,
        ) -> Result<HttpResponseOk<UserV1>, HttpError>;

        /// Get a specific user by ID with profile data (v2+).
        #[endpoint {
            method = GET,
            path = "/users/{id}",
            operation_id = "get_user",
            versions = "2.0.0"..
        }]
        async fn get_user_v2(
            rqctx: RequestContext<Self::Context>,
            path: Path<UserIdPath>,
        ) -> Result<HttpResponseOk<UserV2>, HttpError>;

        /// Create a new user (v1 only).
        #[endpoint {
            method = POST,
            path = "/users",
            operation_id = "create_user",
            versions = "1.0.0"..VERSION_WITH_PROFILE_DATA
        }]
        async fn create_user_v1(
            rqctx: RequestContext<Self::Context>,
            body: TypedBody<CreateUserRequestV1>,
        ) -> Result<HttpResponseOk<UserV1>, HttpError>;

        /// Create a new user with profile data (v2+).
        #[endpoint {
            method = POST,
            path = "/users",
            operation_id = "create_user",
            versions = "2.0.0"..
        }]
        async fn create_user_v2(
            rqctx: RequestContext<Self::Context>,
            body: TypedBody<CreateUserRequestV2>,
        ) -> Result<HttpResponseOk<UserV2>, HttpError>;

        /// Assign role to user (v3+).
        #[endpoint {
            method = PUT,
            path = "/users/{id}/role",
            operation_id = "assign_user_role",
            versions = "3.0.0"..
        }]
        async fn assign_user_role(
            rqctx: RequestContext<Self::Context>,
            path: Path<UserIdPath>,
            body: TypedBody<AssignRoleRequest>,
        ) -> Result<HttpResponseOk<UserV2>, HttpError>;

        /// List user permissions (v3+).
        #[endpoint {
            method = GET,
            path = "/users/{id}/permissions",
            operation_id = "list_user_permissions",
            versions = "3.0.0"..
        }]
        async fn list_user_permissions(
            rqctx: RequestContext<Self::Context>,
            path: Path<UserIdPath>,
        ) -> Result<HttpResponseOk<UserPermissions>, HttpError>;
    }

    /// Query parameters for listing users (v1).
    #[derive(JsonSchema, Deserialize)]
    pub struct ListUsersQueryV1 {
        pub limit: Option<u32>,
        pub offset: Option<u32>,
    }

    /// Query parameters for listing users (v2+).
    #[derive(JsonSchema, Deserialize)]
    pub struct ListUsersQueryV2 {
        pub limit: Option<u32>,
        pub offset: Option<u32>,
        pub email_filter: Option<String>,
        pub include_inactive: Option<bool>,
    }

    /// User information (v1).
    #[derive(JsonSchema, Serialize)]
    pub struct UserV1 {
        pub id: u32,
        pub name: String,
        pub email: String,
        pub created_at: DateTime<Utc>,
    }

    /// User information with profile data (v2+).
    #[derive(JsonSchema, Serialize)]
    pub struct UserV2 {
        pub id: u32,
        pub name: String,
        pub email: String,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub profile: UserProfile,
        pub is_active: bool,
        pub role: Option<String>, // Added in v3 but wire-compatible.
    }

    /// User profile information (v2+).
    #[derive(JsonSchema, Serialize)]
    pub struct UserProfile {
        pub bio: Option<String>,
        pub avatar_url: Option<String>,
        pub timezone: Option<String>,
        pub preferred_language: Option<String>,
    }

    /// List of users response (v1).
    #[derive(JsonSchema, Serialize)]
    pub struct UserListV1 {
        pub users: Vec<UserV1>,
        pub total_count: u32,
    }

    /// List of users response (v2+).
    #[derive(JsonSchema, Serialize)]
    pub struct UserListV2 {
        pub users: Vec<UserV2>,
        pub total_count: u32,
        pub has_more: bool,
    }

    /// Request to create a new user (v1).
    #[derive(JsonSchema, Deserialize)]
    pub struct CreateUserRequestV1 {
        pub name: String,
        pub email: String,
    }

    /// Request to create a new user with profile (v2+).
    #[derive(JsonSchema, Deserialize)]
    pub struct CreateUserRequestV2 {
        pub name: String,
        pub email: String,
        pub profile: Option<CreateUserProfile>,
    }

    /// Profile data for user creation (v2+).
    #[derive(JsonSchema, Deserialize)]
    pub struct CreateUserProfile {
        pub bio: Option<String>,
        pub timezone: Option<String>,
        pub preferred_language: Option<String>,
    }

    /// Request to assign role to user (v3+).
    #[derive(JsonSchema, Deserialize)]
    pub struct AssignRoleRequest {
        pub role: String,
    }

    /// User permissions response (v3+).
    #[derive(JsonSchema, Serialize)]
    pub struct UserPermissions {
        pub user_id: u32,
        pub role: String,
        pub permissions: Vec<String>,
    }
}

/// Reduced versioned health API for testing version removal scenarios.
pub mod versioned_health_reduced {
    use super::*;
    use dropshot_api_manager_types::api_versions;

    api_versions!([(2, WITH_DETAILED_STATUS), (1, INITIAL),]);

    #[dropshot::api_description]
    pub trait VersionedHealthApi {
        type Context;

        /// Check if the service is healthy (all versions).
        #[endpoint {
            method = GET,
            path = "/health",
            operation_id = "health_check",
            versions = "1.0.0"..
        }]
        async fn health_check(
            rqctx: RequestContext<Self::Context>,
        ) -> Result<HttpResponseOk<HealthStatusV1>, HttpError>;

        /// Get detailed health status (v2+).
        #[endpoint {
            method = GET,
            path = "/health/detailed",
            operation_id = "detailed_health_check",
            versions = "2.0.0"..
        }]
        async fn detailed_health_check(
            rqctx: RequestContext<Self::Context>,
        ) -> Result<HttpResponseOk<DetailedHealthStatus>, HttpError>;
    }

    // Reuse the same response types from the main versioned_health module.
    pub use super::versioned_health::{
        DependencyStatus, DetailedHealthStatus, HealthStatusV1,
    };
}
