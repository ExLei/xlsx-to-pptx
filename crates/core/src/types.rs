use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct WordEntry {
    pub word: String,
    pub phonetic: String,
    pub morphology: String,
    pub example: String,
    pub example_definition: String,
    pub definition: String,
}

impl WordEntry {
    /// 返回 true 如果此条应该跳过（word 为空或全空）
    pub fn should_skip(&self) -> bool {
        self.word.trim().is_empty()
            || (self.word.is_empty()
                && self.phonetic.is_empty()
                && self.morphology.is_empty()
                && self.example.is_empty()
                && self.example_definition.is_empty()
                && self.definition.is_empty())
    }
}

#[derive(Debug, Clone)]
pub enum InputSource {
    Excel { path: PathBuf, sheet: String },
    Csv { path: PathBuf, encoding: String },
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("缺少列: {0:?}")]
    MissingColumns(Vec<String>),
    #[error("第 {row} 行必填字段为空: {field}")]
    EmptyRequiredField { row: usize, field: String },
    #[error("文件格式无效: {0}")]
    InvalidFormat(String),
    #[error("无法打开文件: {0}")]
    IoError(#[from] std::io::Error),
    #[error("编码错误: {0}")]
    EncodingError(String),
    #[error("Excel 读取错误: {0}")]
    ExcelError(String),
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("没有可生成的条目")]
    NoEntries,
    #[error("文件已存在: {0}")]
    FileExists(String),
    #[error("已取消")]
    Cancelled,
    #[error("PPTX 生成错误: {0}")]
    PptxError(String),
    #[error("模板错误: {0}")]
    TemplateError(#[from] TemplateError),
    #[error("IO 错误: {0}")]
    IoError(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("无法写入模板: {0}")]
    IoError(#[from] std::io::Error),
    #[error("内置模板损坏")]
    CorruptEmbedded,
    #[error("模板文件损坏或不是有效的 PPTX: {0}")]
    Corrupt(String),
    #[error("模板缺少必填占位符: {{单词}}")]
    MissingPlaceholder,
}
