#![expect(
    dead_code,
    reason = "serde boundary types are validated by deserialization only"
)]

use async_openai::types::chat::CreateChatCompletionRequest;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

pub(crate) fn validate_chat_completion_request(value: Value) -> Result<(), ValidationError> {
    validate_openai_chat_completion_request(value.clone())?;
    validate_nested_chat_completion_fields(value).map_err(ValidationError::InvalidSchema)
}

fn validate_openai_chat_completion_request(value: Value) -> Result<(), ValidationError> {
    let mut ignored_fields = Vec::new();
    let _: CreateChatCompletionRequest = serde_ignored::deserialize(value, |path| {
        ignored_fields.push(path.to_string());
    })
    .map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            ValidationError::InvalidJson(error)
        } else {
            ValidationError::InvalidSchema(error)
        }
    })?;

    if let Some(field) = ignored_fields.into_iter().next() {
        return Err(ValidationError::UnsupportedField { path: field });
    }

    Ok(())
}

fn validate_nested_chat_completion_fields(value: Value) -> Result<(), serde_json::Error> {
    serde_json::from_value::<NestedChatCompletionFields>(value).map(|_| ())
}

#[derive(Debug)]
pub(crate) enum ValidationError {
    InvalidJson(serde_json::Error),
    InvalidSchema(serde_json::Error),
    UnsupportedField { path: String },
}

#[derive(Debug, Deserialize)]
struct NestedChatCompletionFields {
    messages: Option<Vec<StrictMessage>>,
    response_format: Option<StrictResponseFormat>,
    tool_choice: Option<StrictToolChoice>,
    tools: Option<Vec<StrictTool>>,
    #[serde(flatten)]
    _other: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum StrictMessage {
    Developer(StrictDeveloperMessage),
    System(StrictSystemMessage),
    User(StrictUserMessage),
    Assistant(StrictAssistantMessage),
    Tool(StrictToolMessage),
    Function(StrictFunctionMessage),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictDeveloperMessage {
    content: Option<StrictTextContent>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictSystemMessage {
    content: Option<StrictTextContent>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserMessage {
    content: Option<StrictUserContent>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAssistantMessage {
    content: Option<StrictAssistantContent>,
    refusal: Option<String>,
    name: Option<String>,
    audio: Option<StrictAssistantAudio>,
    tool_calls: Option<Vec<StrictToolCall>>,
    function_call: Option<StrictFunctionCall>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictToolMessage {
    content: Option<StrictTextContent>,
    tool_call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFunctionMessage {
    content: Option<String>,
    name: Option<String>,
}

#[derive(Debug)]
enum StrictTextContent {
    Text(String),
    Parts(Vec<StrictTextPart>),
}

#[derive(Debug)]
enum StrictUserContent {
    Text(String),
    Parts(Vec<StrictUserContentPart>),
}

#[derive(Debug)]
enum StrictAssistantContent {
    Text(String),
    Parts(Vec<StrictAssistantContentPart>),
}

impl<'de> Deserialize<'de> for StrictTextContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if value.is_string() {
            serde_json::from_value(value)
                .map(Self::Text)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Parts)
                .map_err(D::Error::custom)
        }
    }
}

impl<'de> Deserialize<'de> for StrictUserContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if value.is_string() {
            serde_json::from_value(value)
                .map(Self::Text)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Parts)
                .map_err(D::Error::custom)
        }
    }
}

impl<'de> Deserialize<'de> for StrictAssistantContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if value.is_string() {
            serde_json::from_value(value)
                .map(Self::Text)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Parts)
                .map_err(D::Error::custom)
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum StrictTextPart {
    Text(StrictTextPartFields),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum StrictUserContentPart {
    Text(StrictTextPartFields),
    ImageUrl(StrictImagePartFields),
    InputAudio(StrictAudioPartFields),
    File(StrictFilePartFields),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum StrictAssistantContentPart {
    Text(StrictTextPartFields),
    Refusal(StrictRefusalPartFields),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictTextPartFields {
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictRefusalPartFields {
    refusal: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictImagePartFields {
    image_url: StrictImageUrl,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictImageUrl {
    url: String,
    detail: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAudioPartFields {
    input_audio: StrictInputAudio,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictInputAudio {
    data: String,
    format: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFilePartFields {
    file: StrictFileObject,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFileObject {
    file_data: Option<String>,
    file_id: Option<String>,
    filename: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAssistantAudio {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum StrictToolCall {
    Function(StrictFunctionToolCall),
    Custom(StrictCustomToolCall),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFunctionToolCall {
    id: String,
    function: StrictFunctionCall,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCustomToolCall {
    id: String,
    custom_tool: StrictCustomTool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCustomTool {
    name: String,
    input: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum StrictTool {
    Function(StrictFunctionTool),
    Custom(StrictCustomToolDefinition),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFunctionTool {
    function: StrictFunctionObject,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFunctionObject {
    name: String,
    description: Option<String>,
    parameters: Option<Value>,
    strict: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCustomToolDefinition {
    custom: StrictCustomToolProperties,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCustomToolProperties {
    name: String,
    description: Option<String>,
    format: Option<Value>,
}

#[derive(Debug)]
enum StrictToolChoice {
    Object(StrictToolChoiceObject),
    Mode(StrictToolChoiceMode),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum StrictToolChoiceObject {
    AllowedTools(StrictAllowedToolsChoice),
    Function(StrictNamedFunctionToolChoice),
    Custom(StrictNamedCustomToolChoice),
}

impl<'de> Deserialize<'de> for StrictToolChoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if value.is_string() {
            serde_json::from_value(value)
                .map(Self::Mode)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Object)
                .map_err(D::Error::custom)
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAllowedToolsChoice {
    allowed_tools: Vec<StrictAllowedTools>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAllowedTools {
    mode: String,
    tools: Vec<StrictTool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictNamedFunctionToolChoice {
    function: StrictFunctionName,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictNamedCustomToolChoice {
    custom: StrictCustomName,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFunctionName {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCustomName {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum StrictToolChoiceMode {
    None,
    Auto,
    Required,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum StrictResponseFormat {
    Text,
    JsonObject,
    JsonSchema {
        json_schema: StrictResponseFormatJsonSchema,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictResponseFormatJsonSchema {
    description: Option<String>,
    name: String,
    schema: Option<Value>,
    strict: Option<bool>,
}
