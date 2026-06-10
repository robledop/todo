pub const APP_ID: &str = "dev.robledo.OutlookTasks";

/// Graph API root (no trailing slash).
pub const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Public-client app (registration) id. Override at build time with
/// `OUTLOOK_TASKS_CLIENT_ID=...`; otherwise the placeholder is compiled in.
pub const CLIENT_ID: &str = match option_env!("OUTLOOK_TASKS_CLIENT_ID") {
    Some(id) => id,
    None => "REPLACE_WITH_YOUR_CLIENT_ID",
};

/// Single account slot id used for the keyring attribute and account id.
pub const ACCOUNT_ID: &str = "primary";
