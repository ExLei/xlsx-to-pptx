//! 输入适配器：xlsx（calamine）/ csv（csv + encoding_rs）。
//!
//! 两个格式共用同一条流水线：表头校验（对照 [`crate::domain::schema::FIELDS`]）
//! → 逐行解析 → 跳过「英文单词为空」的行（记 warn + 计数）。
//!
//! 对等旧版（`xlsx-to-pptx-legacy/crates/core/src/reader.rs`）之处：列按表头名映射、
//! 未知列忽略、单元格按文本取值后 trim、行号从 2 起算（含表头行）、GBK/GB2312 容错解码、
//! 错误文案。差别只在错误类型：旧版把它们压成 `String`，这里用 [`LoadError`] 变体，
//! 调用方能够分层处理（缺列 / 行级 / 编码 / 空表）。

use std::borrow::Cow;
use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};
use serde_json::json;

use crate::diag::{field, Diag, Target};
use crate::domain::schema::{Field, SheetInfo, Source, SourceKind, WordEntry, FIELDS};

/// 读取结果。
#[derive(Debug, Clone)]
pub struct ReadResult {
    pub entries: Vec<WordEntry>,
    pub skipped: usize,
    pub columns: Vec<Field>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("无法打开文件: {0}")]
    Open(String),
    #[error("缺少列: {0:?}")]
    MissingColumns(Vec<&'static str>),
    #[error("第 {row} 行解析失败: {detail}")]
    RowParse { row: usize, detail: String },
    #[error("编码错误: {label}")]
    Encoding { label: String },
    #[error("工作表为空或没有表头")]
    EmptySheet,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 列出工作表（Excel）：按工作簿内的顺序。
pub fn list_sheets(path: &Path, diag: &Diag) -> Result<Vec<SheetInfo>, LoadError> {
    let result = sheet_names(path);
    match &result {
        Ok(sheets) => diag.info(
            Target::Sheets,
            format!("发现 {} 个工作表", sheets.len()),
            Some(json!({ field::PATH: display(path), field::COUNT: sheets.len() })),
        ),
        Err(error) => diag.error(
            Target::Sheets,
            format!("打开工作簿失败: {error}"),
            Some(json!({ field::PATH: display(path) })),
        ),
    }
    result
}

/// 读取词条；`SourceKind::Excel { sheet: None }` 时取第一个工作表。
pub fn read(source: &Source, diag: &Diag) -> Result<ReadResult, LoadError> {
    let result = match &source.kind {
        SourceKind::Excel { sheet } => read_excel(&source.path, sheet.as_deref(), diag),
        SourceKind::Csv { encoding } => read_csv(&source.path, encoding, diag),
    };
    // 零静默失败（I2）：每个失败出口都在这里留痕（内含缺失列名等结构信息）。
    if let Err(error) = &result {
        let fields = match error {
            LoadError::MissingColumns(missing) => {
                json!({ field::PATH: display(&source.path), field::MISSING: missing })
            }
            _ => json!({ field::PATH: display(&source.path) }),
        };
        diag.error(Target::Sheets, format!("读取失败: {error}"), Some(fields));
    }
    result
}

/// 打开工作簿取工作表名。`open_workbook_auto` 认扩展名：xlsx / xls / xlsb / ods。
fn sheet_names(path: &Path) -> Result<Vec<SheetInfo>, LoadError> {
    let workbook = open_workbook_auto(path)
        .map_err(|error| LoadError::Open(format!("{}: {error}", display(path))))?;
    let sheets: Vec<SheetInfo> = workbook
        .sheet_names()
        .into_iter()
        .map(|name| SheetInfo { name })
        .collect();
    if sheets.is_empty() {
        return Err(LoadError::Open(format!(
            "{}: 工作簿没有任何工作表",
            display(path)
        )));
    }
    Ok(sheets)
}

/// Excel：选表 → 表头 → 共用流水线。
fn read_excel(path: &Path, sheet: Option<&str>, diag: &Diag) -> Result<ReadResult, LoadError> {
    let mut workbook = open_workbook_auto(path)
        .map_err(|error| LoadError::Open(format!("{}: {error}", display(path))))?;
    let names = workbook.sheet_names();
    let name = match sheet {
        Some(wanted) if names.iter().any(|name| name == wanted) => wanted,
        Some(wanted) => {
            return Err(LoadError::Open(format!(
                "工作表不存在: {wanted}（工作簿内: {}）",
                names.join("、")
            )))
        }
        None => names
            .first()
            .map(String::as_str)
            .ok_or_else(|| LoadError::Open(format!("{}: 工作簿没有任何工作表", display(path))))?,
    };
    diag.info(
        Target::Sheets,
        format!("读取工作表 {name}"),
        Some(json!({ field::PATH: display(path), field::SHEET: name })),
    );

    let range = workbook
        .worksheet_range(name)
        .map_err(|error| LoadError::Open(format!("无法读取工作表 {name}: {error}")))?;
    let mut rows = range.rows();
    let header = rows.next().ok_or(LoadError::EmptySheet)?;
    let headers = headers_of(header)?;
    let read = parse_rows(
        &headers,
        rows.enumerate().map(|(index, row)| (index + 2, Ok(row))),
        diag,
    )?;

    diag.info(
        Target::Sheets,
        format!(
            "工作表 {name} 读取完成: {} 条词条（跳过 {} 行）",
            read.entries.len(),
            read.skipped
        ),
        Some(json!({
            field::PATH: display(path),
            field::SHEET: name,
            field::ENTRIES: read.entries.len(),
            field::SKIPPED: read.skipped,
            field::COLUMNS: column_headers(&read),
        })),
    );
    Ok(read)
}

/// CSV：编码标签 → 解码 → 表头 → 共用流水线。
fn read_csv(path: &Path, label: &str, diag: &Diag) -> Result<ReadResult, LoadError> {
    // 标签无效立即报错，不静默回退（design §6.2）。
    let encoding = encoding_rs::Encoding::for_label(label.trim().as_bytes()).ok_or_else(|| {
        LoadError::Encoding {
            label: label.to_string(),
        }
    })?;
    let raw = std::fs::read(path)
        .map_err(|error| LoadError::Open(format!("{}: {error}", display(path))))?;
    let text = decode(encoding, label, &raw, diag)?;
    diag.info(
        Target::Sheets,
        format!("读取 csv（{label}）"),
        Some(json!({ field::PATH: display(path), field::ENCODING: label })),
    );

    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let headers = headers_of(reader.headers().map_err(|error| LoadError::RowParse {
        row: 1,
        detail: format!("表头解析失败: {error}"),
    })?)?;
    let rows = reader.records().enumerate().map(|(index, record)| {
        let row = index + 2;
        (
            row,
            record.map_err(|error| LoadError::RowParse {
                row,
                detail: error.to_string(),
            }),
        )
    });
    let read = parse_rows(&headers, rows, diag)?;

    diag.info(
        Target::Sheets,
        format!(
            "csv 读取完成: {} 条词条（跳过 {} 行）",
            read.entries.len(),
            read.skipped
        ),
        Some(json!({
            field::PATH: display(path),
            field::ENCODING: label,
            field::ENTRIES: read.entries.len(),
            field::SKIPPED: read.skipped,
            field::COLUMNS: column_headers(&read),
        })),
    );
    Ok(read)
}

// ---------------------------------------------------------------- 共用流水线

/// 共用流水线：表头 → 列映射 → 逐行词条 → 跳过计数。
///
/// `rows` 产出 `(行号, 行)`，行号从 1 起且含表头行（对等旧版文案）。
/// 行级问题（csv 记录错误、xlsx 公式错误单元格）→ [`LoadError::RowParse`]。
fn parse_rows<R: RowCells>(
    headers: &[String],
    rows: impl Iterator<Item = (usize, Result<R, LoadError>)>,
    diag: &Diag,
) -> Result<ReadResult, LoadError> {
    let layout = ColumnLayout::resolve(headers)?;
    let mut entries = Vec::new();
    let mut skipped = 0usize;
    for (number, row) in rows {
        let row = row?;
        let entry = layout.entry(&row);
        if entry.is_skippable() {
            // 跳过优先：被跳过的行不产出数据，它的单元格问题不牵连整份读取。
            skipped += 1;
            diag.warn(
                Target::Sheets,
                format!("第 {number} 行「英文单词」为空，已跳过"),
                Some(json!({ field::ROW: number })),
            );
            continue;
        }
        if let Some(detail) = row_error(&row, &layout) {
            return Err(LoadError::RowParse { row: number, detail });
        }
        entries.push(entry);
    }
    Ok(ReadResult {
        entries,
        skipped,
        columns: layout.fields,
    })
}

/// 表头 → 列索引（未知列忽略；下标 = `Field::index()`）。
struct ColumnLayout {
    index: [usize; FIELDS.len()],
    /// 按 [`FIELDS`] 顺序识别出的字段。
    fields: Vec<Field>,
}

impl ColumnLayout {
    /// 解析表头：缺列一次性聚合、按 [`FIELDS`] 顺序报出。
    fn resolve(headers: &[String]) -> Result<ColumnLayout, LoadError> {
        let mut index = [0usize; FIELDS.len()];
        let mut fields = Vec::with_capacity(FIELDS.len());
        let mut missing: Vec<&'static str> = Vec::new();
        for meta in FIELDS.iter() {
            match headers
                .iter()
                .position(|header| header.trim() == meta.header)
            {
                Some(column) => {
                    index[meta.field.index()] = column;
                    fields.push(meta.field);
                }
                None => missing.push(meta.header),
            }
        }
        if !missing.is_empty() {
            return Err(LoadError::MissingColumns(missing));
        }
        Ok(ColumnLayout { index, fields })
    }

    /// 一行 → 词条（单元格 trim；越界列按空处理）。
    fn entry<R: RowCells>(&self, row: &R) -> WordEntry {
        let mut entry = WordEntry::default();
        for meta in FIELDS.iter() {
            let value = row.cell(self.index[meta.field.index()]).trim().to_string();
            set(&mut entry, meta.field, value);
        }
        entry
    }
}

/// 按字段写入（`WordEntry` 的字段顺序 = `Field::index()` 顺序）。
fn set(entry: &mut WordEntry, field: Field, value: String) {
    match field {
        Field::Word => entry.word = value,
        Field::Phonetic => entry.phonetic = value,
        Field::Morphology => entry.morphology = value,
        Field::Example => entry.example = value,
        Field::ExampleDef => entry.example_definition = value,
        Field::Definition => entry.definition = value,
    }
}

/// 行级问题：映射列里出现 `#REF!` 之类的错误值（未映射的列忽略）。
fn row_error<R: RowCells>(row: &R, layout: &ColumnLayout) -> Option<String> {
    FIELDS.iter().find_map(|meta| {
        row.cell_error(layout.index[meta.field.index()])
            .map(|error| format!("列「{}」的单元格为错误值 {error}", meta.header))
    })
}

/// 首行表头 → 列名；空行/无表头 → [`LoadError::EmptySheet`]。
fn headers_of<R: RowCells + ?Sized>(row: &R) -> Result<Vec<String>, LoadError> {
    let headers: Vec<String> = (0..row.width())
        .map(|index| row.cell(index).into_owned())
        .collect();
    if headers.iter().all(|header| header.trim().is_empty()) {
        return Err(LoadError::EmptySheet);
    }
    Ok(headers)
}

/// 行级取值：xlsx 单元格 / csv 记录——流水线唯一的格式差异点。
trait RowCells {
    /// 列数。
    fn width(&self) -> usize;
    /// 第 `index` 列的文本（越界或空单元格 → 空串；已 trim）。
    fn cell(&self, index: usize) -> Cow<'_, str>;
    /// 第 `index` 列的错误值文案（csv 无此概念 → `None`）。
    fn cell_error(&self, _index: usize) -> Option<String> {
        None
    }
}

impl RowCells for [Data] {
    fn width(&self) -> usize {
        self.len()
    }

    fn cell(&self, index: usize) -> Cow<'_, str> {
        match self.get(index) {
            Some(Data::String(text))
            | Some(Data::DateTimeIso(text))
            | Some(Data::DurationIso(text)) => Cow::Borrowed(text.trim()),
            Some(Data::Int(value)) => Cow::Owned(value.to_string()),
            Some(Data::Float(value)) => Cow::Owned(value.to_string()),
            Some(Data::Bool(value)) => Cow::Owned(value.to_string()),
            Some(Data::DateTime(value)) => Cow::Owned(value.to_string()),
            Some(Data::Error(error)) => Cow::Owned(error.to_string()),
            Some(Data::Empty) | None => Cow::Borrowed(""),
        }
    }

    fn cell_error(&self, index: usize) -> Option<String> {
        match self.get(index) {
            Some(Data::Error(error)) => Some(error.to_string()),
            _ => None,
        }
    }
}

impl RowCells for csv::StringRecord {
    fn width(&self) -> usize {
        self.len()
    }

    fn cell(&self, index: usize) -> Cow<'_, str> {
        // `Trim::All` 已在解析时去空白；这里再 trim 一次，流水线只依赖 trim 后的文本。
        self.get(index)
            .map_or(Cow::Borrowed(""), |text| Cow::Borrowed(text.trim()))
    }
}

impl<T: RowCells + ?Sized> RowCells for &T {
    fn width(&self) -> usize {
        (**self).width()
    }

    fn cell(&self, index: usize) -> Cow<'_, str> {
        (**self).cell(index)
    }

    fn cell_error(&self, index: usize) -> Option<String> {
        (**self).cell_error(index)
    }
}

// ------------------------------------------------------------------ 小工具

/// 解码：`encoding_rs` 的 `decode` 自身不失败（无效字节换成 U+FFFD），严格性由这里定：
/// UTF-8 出现无效字节按旧版严格分支报错；其余编码对等旧版容错（替换 + warn），
/// GBK/GB2312 因此不会整份读不出。BOM 由 `decode` 去掉
/// （旧版 UTF-8 分支用 `from_utf8` 保留 BOM → 表头失配，此处一并修正）。
fn decode<'a>(
    encoding: &'static encoding_rs::Encoding,
    label: &str,
    raw: &'a [u8],
    diag: &Diag,
) -> Result<Cow<'a, str>, LoadError> {
    let (text, _, had_errors) = encoding.decode(raw);
    if !had_errors {
        return Ok(text);
    }
    if encoding == encoding_rs::UTF_8 {
        return Err(LoadError::Encoding {
            label: label.to_string(),
        });
    }
    diag.warn(
        Target::Sheets,
        format!("编码 {label} 解码遇到无效字节，已替换"),
        Some(json!({ field::ENCODING: label })),
    );
    Ok(text)
}

/// diag `columns` 字段：识别出的列名（顺序按 [`FIELDS`]）。
fn column_headers(read: &ReadResult) -> Vec<&'static str> {
    read.columns.iter().map(|field| field.header()).collect()
}

/// 路径的文案形态。
fn display(path: &Path) -> String {
    path.display().to_string()
}
