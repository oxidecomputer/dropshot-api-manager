// Copyright 2025 Oxide Computer Company

//! Test fixtures for common API scenarios in dropshot-api-manager tests.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use dropshot::{
    HttpError, HttpResponseOk, Path, Query, RequestContext, TypedBody,
};
use dropshot_api_manager::{ManagedApiConfig, ManagedApis};
use dropshot_api_manager_types::{
    ManagedApiMetadata, ValidationContext, Versions,
};
use schemars::JsonSchema;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

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

    api_versions!([(3, WITH_METRICS), (2, WITH_DETAILED_STATUS), (1, INITIAL)]);

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

    api_versions!([(2, WITH_DETAILED_STATUS), (1, INITIAL)]);

    #[dropshot::api_description { module = "api_mod" }]
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

/// Versioned health API fixture that skips the middle version (2.0.0).
/// This has versions 3.0.0 and 1.0.0 only, simulating retirement of an older
/// blessed version.
pub mod versioned_health_skip_middle {
    use super::*;
    use dropshot_api_manager_types::api_versions;

    api_versions!([(3, WITH_METRICS), (1, INITIAL)]);

    #[dropshot::api_description { module = "api_mod" }]
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

        /// Get detailed health status (v2+, but only available in v3 since we
        /// skip v2).
        #[endpoint {
            method = GET,
            path = "/health/detailed",
            operation_id = "detailed_health_check",
            versions = "3.0.0"..
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

    // Reuse the same response types from the main versioned_health module.
    pub use super::versioned_health::{
        DependencyStatus, DetailedHealthStatus, HealthStatusV1, ServiceMetrics,
    };
}

/// Versioned health API with incompatible changes - this breaks backward
/// compatibility by changing the response schema of an existing endpoint.
pub mod versioned_health_incompat {
    use super::*;
    use dropshot_api_manager_types::api_versions;

    api_versions!([(3, WITH_METRICS), (2, WITH_DETAILED_STATUS), (1, INITIAL)]);

    #[dropshot::api_description { module = "api_mod" }]
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

        /// Get system info (v3+): new endpoint added to existing version.
        ///
        /// This breaks backward compatibility by adding a new endpoint to
        /// v3.0.0.
        #[endpoint {
            method = GET,
            path = "/system/info",
            operation_id = "get_system_info",
            versions = "3.0.0"..
        }]
        async fn get_system_info(
            rqctx: RequestContext<Self::Context>,
        ) -> Result<HttpResponseOk<SystemInfo>, HttpError>;
    }

    /// System information response for the new endpoint.
    #[derive(JsonSchema, Serialize)]
    pub struct SystemInfo {
        pub version: String,
        pub build_time: DateTime<Utc>,
        pub environment: String,
    }

    // Reuse response types from the main versioned_health module for other
    // endpoints.
    pub use super::versioned_health::{
        DependencyStatus, DetailedHealthStatus, HealthStatusV1, ServiceMetrics,
    };
}

pub fn versioned_health_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            supported_versions: versioned_health::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some(
                "A versioned health API for testing version evolution",
            ),
            ..Default::default()
        },
        api_description:
            versioned_health::versioned_health_api_mod::stub_api_description,
        extra_validation: None,
    }
}

pub fn versioned_user_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "versioned-user",
        versions: Versions::Versioned {
            supported_versions: versioned_user::supported_versions(),
        },
        title: "Versioned User API",
        metadata: ManagedApiMetadata {
            description: Some(
                "A versioned user API for testing complex schema evolution",
            ),
            ..Default::default()
        },
        api_description:
            versioned_user::versioned_user_api_mod::stub_api_description,
        extra_validation: None,
    }
}

pub fn lockstep_health_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "health",
        versions: Versions::Lockstep { version: Version::new(1, 0, 0) },
        title: "Health API",
        metadata: ManagedApiMetadata {
            description: Some("A health API for testing schema evolution"),
            ..Default::default()
        },
        api_description: health_api_mod::stub_api_description,
        extra_validation: None,
    }
}

pub fn lockstep_counter_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "counter",
        versions: Versions::Lockstep { version: Version::new(1, 0, 0) },
        title: "Counter Test API",
        metadata: ManagedApiMetadata {
            description: Some("A counter API for testing state changes"),
            ..Default::default()
        },
        api_description: counter_api_mod::stub_api_description,
        extra_validation: None,
    }
}

pub fn lockstep_user_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "user",
        versions: Versions::Lockstep { version: Version::new(1, 0, 0) },
        title: "User Test API",
        metadata: ManagedApiMetadata {
            description: Some("A user API for testing state changes"),
            ..Default::default()
        },
        api_description: user_api_mod::stub_api_description,
        extra_validation: None,
    }
}

/// Create a health API for basic testing.
pub fn lockstep_health_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![lockstep_health_api()])
        .context("failed to create ManagedApis")
}

/// Create a counter test API configuration.
pub fn lockstep_counter_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![lockstep_counter_api()])
        .context("failed to create ManagedApis")
}

/// Create a user test API configuration.
pub fn lockstep_user_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![lockstep_user_api()])
        .context("failed to create ManagedApis")
}

/// Helper to create multiple test APIs.
pub fn lockstep_multi_apis() -> Result<ManagedApis> {
    let configs = vec![
        lockstep_health_api(),
        lockstep_counter_api(),
        lockstep_user_api(),
    ];
    ManagedApis::new(configs).context("failed to create ManagedApis")
}

/// Create a versioned health API for testing.
pub fn versioned_health_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![versioned_health_api()])
        .context("failed to create versioned health ManagedApis")
}

/// Create a versioned user API for testing.
pub fn versioned_user_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![versioned_user_api()])
        .context("failed to create versioned user ManagedApis")
}

/// Helper to create multiple versioned test APIs.
pub fn multi_versioned_apis() -> Result<ManagedApis> {
    let configs = vec![versioned_health_api(), versioned_user_api()];
    ManagedApis::new(configs).context("failed to create versioned ManagedApis")
}

/// Helper to create mixed lockstep and versioned test APIs.
pub fn create_mixed_test_apis() -> Result<ManagedApis> {
    let configs = vec![
        lockstep_health_api(),
        lockstep_counter_api(),
        versioned_health_api(),
        versioned_user_api(),
    ];
    ManagedApis::new(configs).context("failed to create mixed ManagedApis")
}

/// Create versioned health API with a trivial change (title/metadata updated).
pub fn versioned_health_trivial_change_apis() -> Result<ManagedApis> {
    // Create a modified API config that would produce different OpenAPI
    // documents.
    let mut config = versioned_health_api();

    // Modify the title to create a different document signature.
    config.title = "Modified Versioned Health API";
    config.metadata.description =
        Some("A versioned health API with breaking changes");

    ManagedApis::new(vec![config])
        .context("failed to create trivial change versioned health ManagedApis")
}

/// Create versioned health API with reduced versions (simulating version
/// removal).
pub fn versioned_health_reduced_apis() -> Result<ManagedApis> {
    // Create a configuration similar to versioned health but with fewer
    // versions. We'll create a new fixture for this.
    let config = ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            // Use a subset of versions (only 1.0.0 and 2.0.0, not 3.0.0).
            supported_versions: versioned_health_reduced::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some("A versioned health API with reduced versions"),
            ..Default::default()
        },
        api_description:
            versioned_health_reduced::api_mod::stub_api_description,
        extra_validation: None,
    };

    ManagedApis::new(vec![config])
        .context("failed to create reduced versioned health ManagedApis")
}

pub fn versioned_health_skip_middle_apis() -> Result<ManagedApis> {
    // Create a configuration similar to versioned health but skipping the
    // middle version. This has versions 3.0.0 and 1.0.0, simulating retirement
    // of version 2.0.0.
    let config = ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            // Use versions 3.0.0 and 1.0.0 (skip 2.0.0).
            supported_versions:
                versioned_health_skip_middle::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some(
                "A versioned health API that skips middle version",
            ),
            ..Default::default()
        },
        api_description:
            versioned_health_skip_middle::api_mod::stub_api_description,
        extra_validation: None,
    };

    ManagedApis::new(vec![config])
        .context("failed to create skip middle versioned health ManagedApis")
}

/// Create a versioned health API with incompatible changes that break backward
/// compatibility.
pub fn versioned_health_incompat_apis() -> Result<ManagedApis> {
    // Create a configuration similar to versioned health but with incompatible
    // changes that break backward compatibility.
    let config = ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            supported_versions: versioned_health_incompat::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some(
                "A versioned health API with incompatible changes",
            ),
            ..Default::default()
        },
        api_description:
            versioned_health_incompat::api_mod::stub_api_description,
        extra_validation: None,
    };

    ManagedApis::new(vec![config])
        .context("failed to create incompatible versioned health ManagedApis")
}

#[derive(Debug, Clone)]
pub struct ValidationCall {
    pub version: Version,
    pub is_latest: bool,
}

// Nextest runs each test in its own process, so this static is isolated per
// test.
static VALIDATION_CALLS: Mutex<Vec<ValidationCall>> = Mutex::new(Vec::new());

pub fn get_validation_calls() -> Vec<ValidationCall> {
    VALIDATION_CALLS.lock().unwrap().clone()
}

pub fn clear_validation_calls() {
    VALIDATION_CALLS.lock().unwrap().clear();
}

fn record_validation_call(version: Version, is_latest: bool) {
    VALIDATION_CALLS
        .lock()
        .unwrap()
        .push(ValidationCall { version, is_latest });
}

fn validate(_spec: &openapiv3::OpenAPI, cx: ValidationContext<'_>) {
    // Only used with versioned APIs, so version is always present.
    let version = cx
        .file_name()
        .version()
        .cloned()
        .expect("version should be present for versioned APIs");

    record_validation_call(version, cx.is_latest());
}

fn validate_with_extra_file(
    _spec: &openapiv3::OpenAPI,
    mut cx: ValidationContext<'_>,
) {
    // Only used with versioned APIs, so version is always present.
    let version = cx
        .file_name()
        .version()
        .cloned()
        .expect("version should be present for versioned APIs");

    record_validation_call(version.clone(), cx.is_latest());

    if cx.is_latest() {
        // Place in the API's directory alongside the OpenAPI documents.
        cx.record_file_contents(
            format!("documents/{}/latest-{}.txt", cx.ident(), version),
            format!("This is the latest version: {}", version).into(),
        );
    }
}

pub fn versioned_health_with_validation_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            supported_versions: versioned_health::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some(
                "A versioned health API with extra validation tracking",
            ),
            ..Default::default()
        },
        api_description:
            versioned_health::versioned_health_api_mod::stub_api_description,
        extra_validation: Some(validate),
    }
}

pub fn versioned_health_with_extra_file_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            supported_versions: versioned_health::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some(
                "A versioned health API with conditional file generation",
            ),
            ..Default::default()
        },
        api_description:
            versioned_health::versioned_health_api_mod::stub_api_description,
        extra_validation: Some(validate_with_extra_file),
    }
}

pub fn versioned_health_with_validation_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![versioned_health_with_validation_api()]).context(
        "failed to create versioned health with validation ManagedApis",
    )
}

pub fn versioned_health_with_extra_file_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![versioned_health_with_extra_file_api()]).context(
        "failed to create versioned health with conditional files ManagedApis",
    )
}
