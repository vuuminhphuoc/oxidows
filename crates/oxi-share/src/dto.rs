//! The JSON bodies of the LocalSend v2 protocol (<https://github.com/localsend/protocol>).
//!
//! Field names and optionality follow what the official app sends and accepts: unknown fields are
//! ignored, optional fields may be missing or `null`, and unknown device types read as desktop.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};

/// The protocol version announced to peers (major.minor).
pub const PROTOCOL_VERSION: &str = "2.1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Http,
    #[default]
    Https,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Https => "https",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Mobile,
    #[default]
    Desktop,
    Web,
    Headless,
    Server,
}

impl<'de> Deserialize<'de> for DeviceType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.to_ascii_lowercase().as_str() {
            "mobile" => DeviceType::Mobile,
            "web" => DeviceType::Web,
            "headless" => DeviceType::Headless,
            "server" => DeviceType::Server,
            _ => DeviceType::Desktop,
        })
    }
}

/// Multicast announcement, register request and the `info` part of prepare-upload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub alias: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<DeviceType>,
    #[serde(default)]
    pub fingerprint: String,
    /// Missing in register responses and `/info`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
    #[serde(default)]
    pub download: bool,
    /// Only in multicast messages: `true` asks others to answer, `false` is an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announce: Option<bool>,
}

fn default_version() -> String {
    "1.0".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessed: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileDto {
    pub id: String,
    pub file_name: String,
    pub size: u64,
    #[serde(default)]
    pub file_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// For a text message: the text itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<FileMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadRequest {
    pub info: DeviceInfo,
    pub files: HashMap<String, FileDto>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadResponse {
    pub session_id: String,
    /// File id to upload token, only for the files the receiver accepted.
    pub files: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareDownloadResponse {
    pub info: DeviceInfo,
    pub session_id: String,
    pub files: HashMap<String, FileDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_announcement_from_the_official_app() {
        let json = r#"{"alias":"Nice Orange","version":"2.0","deviceModel":"Samsung","deviceType":"mobile",
            "fingerprint":"ABC","port":53317,"protocol":"https","download":true,"announce":true}"#;
        let d: DeviceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(d.alias, "Nice Orange");
        assert_eq!(d.device_type, Some(DeviceType::Mobile));
        assert_eq!(d.port, Some(53317));
        assert_eq!(d.protocol, Some(Protocol::Https));
        assert_eq!(d.announce, Some(true));
    }

    #[test]
    fn tolerates_missing_and_unknown_fields() {
        let json = r#"{"alias":"x","deviceType":"fridge","deviceModel":null,"somethingNew":1}"#;
        let d: DeviceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(d.device_type, Some(DeviceType::Desktop));
        assert_eq!(d.device_model, None);
        assert!(!d.download);
        assert_eq!(d.version, "1.0");
    }

    #[test]
    fn writes_camel_case_without_nulls() {
        let f = FileDto {
            id: "1".into(),
            file_name: "a.png".into(),
            size: 3,
            file_type: "image/png".into(),
            sha256: None,
            preview: None,
            metadata: Some(FileMetadata { modified: Some("2021-01-01T12:34:56Z".into()), accessed: None }),
        };
        let s = serde_json::to_string(&f).unwrap();
        assert_eq!(
            s,
            r#"{"id":"1","fileName":"a.png","size":3,"fileType":"image/png","metadata":{"modified":"2021-01-01T12:34:56Z"}}"#
        );
    }
}
