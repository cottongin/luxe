//! Key style — which header a provider's API key goes out in.

use luxe_core::auth::ApiKind;

use super::RequestAuth;
use crate::config::ApiKey;

/// How a provider wants an API key presented.
///
/// Derivable from [`ApiKind`] for every provider that speaks a vendor's own API, and *not*
/// derivable for a host that re-serves someone else's: Microsoft Foundry's Anthropic passthrough
/// speaks Anthropic Messages but authenticates the Azure way. So the style travels with the
/// provider, and a `models.json` block can override it — which is also how a Microsoft Entra ID
/// token reaches a Foundry endpoint, since a bearer token is just a different style over the same
/// `!command` credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyStyle {
    /// `Authorization: Bearer <key>`
    #[default]
    Bearer,
    /// `x-api-key: <key>`
    AnthropicKey,
    /// `api-key: <key>`
    AzureKey,
    /// `x-api-key: <key>` and `api-key: <key>`
    FoundryKey,
}

impl KeyStyle {
    /// The style a provider speaking `api` uses unless it says otherwise.
    pub fn for_api(api: ApiKind) -> Self {
        match api {
            ApiKind::AnthropicMessages => Self::AnthropicKey,
            _ => Self::Bearer,
        }
    }

    /// Parse the `auth` value of a `models.json` provider block.
    ///
    /// Spelled as the header a user reads off a failing request rather than as a vendor name.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "bearer" | "authorization" => Some(Self::Bearer),
            "x-api-key" | "anthropic" => Some(Self::AnthropicKey),
            "api-key" | "azure" => Some(Self::AzureKey),
            "foundry" => Some(Self::FoundryKey),
            _ => None,
        }
    }

    /// The headers `key` goes out as.
    pub fn headers(self, key: &ApiKey) -> RequestAuth {
        match self {
            Self::Bearer => RequestAuth::bearer(key),
            Self::AnthropicKey => RequestAuth::anthropic_key(key),
            Self::AzureKey => RequestAuth::azure_key(key),
            Self::FoundryKey => RequestAuth::foundry_key(key),
        }
    }
}
