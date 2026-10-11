use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::Path;

use calamine::{open_workbook, Data, Reader, Xlsx};

use crate::diag::Diag;
use crate::types::{InputSource, LoadError, WordEntry};

/// 6 必填列: (Excel 表头 → 内部 key)
const REQUIRED_COLUMNS: &[(&str, &str)] = &[
    ("英文单词", "word"),
    ("英文音标", "phonetic"),
    ("词根词缀", "morphology"),
    ("例句", "example"),
    ("例句释义", "example_definition"),
    ("单词释义", "definition"),
];

/// 返回工作簿中所有 sheet 名
pub fn list_sheets(path: &Path, diag: &Diag) -> Result<Vec<String>, LoadError> {
    let workbook: Xlsx<_> =
        open_workbook(path).map_err(|e| LoadError::IoError(std::io::Error::other(e)))?;
    let names = workbook.sheet_names().to_vec();
    diag.info(
        "reader",
        format!("发现 {} 个工作表: {:?}", names.len(), names),
        Some(serde_json::json!({"sheet_count": names.len(), "sheets": names})),
    );
    Ok(names)
}

/// 根据 InputSource 读取并解析词汇数据
pub fn load(source: &InputSource, diag: &Diag) -> Result<Vec<WordEntry>, LoadError> {
    match source {
        InputSource::Excel { path, sheet } => load_excel(path, sheet, diag),
        InputSource::Csv { path, encoding } => load_csv(path, encoding, diag),
    }
}
fn load_excel(path: &Path, sheet: &str, diag: &Diag) -> Result<Vec<WordEntry>, LoadError> {
    let mut workbook: Xlsx<_> =
        open_workbook(path).map_err(|e| LoadError::IoError(std::io::Error::other(e)))?;

    let range = workbook
        .worksheet_range(sheet)
        .map_err(|e| LoadError::ExcelError(format!("无法读取 sheet '{sheet}': {e}")))?;

    // 解析表头行 → 列索引映射
    let headers = extract_headers(&range)?;
    let col_map = build_column_map(&headers)?;

    // 校验 6 必填列 — 一次性报告所有缺失列
    validate_columns(&col_map)?;

    // 逐行解析 (跳过表头行)
    let mut entries = Vec::new();
    for (row_idx, row) in range.rows().skip(1).enumerate() {
        let entry = parse_row(row, &col_map);
        if entry.should_skip() {
            diag.warn(
                "reader",
                format!("第 {} 行 word 为空，已跳过", row_idx + 2),
                Some(serde_json::json!({"row": row_idx + 2})),
            );
            continue;
        }
        entries.push(entry);
    }

    Ok(entries)
}

/// 校验 6 必填列 — 一次性报告所有缺失列
fn validate_columns(col_map: &HashMap<String, usize>) -> Result<(), LoadError> {
    let missing: Vec<String> = REQUIRED_COLUMNS
        .iter()
        .filter(|(header, _)| !col_map.contains_key(*header))
        .map(|(header, _)| header.to_string())
        .collect();
    if !missing.is_empty() {
        return Err(LoadError::MissingColumns(missing));
    }
    Ok(())
}

/// 读取 CSV 文件，支持多编码（UTF-8、GBK、GB2312、GB18030）
fn load_csv(path: &Path, encoding: &str, diag: &Diag) -> Result<Vec<WordEntry>, LoadError> {
    let raw = fs::read(path)?;

    let text = if encoding.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(raw)
            .map_err(|e| LoadError::EncodingError(format!("UTF-8 解码失败: {e}")))?
    } else {
        let enc = encoding_rs::Encoding::for_label(encoding.as_bytes())
            .ok_or_else(|| LoadError::EncodingError(format!("不支持的编码: {encoding}")))?;
        let (cow, _enc_used, had_errors) = enc.decode(&raw);
        if had_errors {
            diag.warn(
                "reader",
                format!("编码 '{encoding}' 解码时遇到无效字节，已替换"),
                Some(serde_json::json!({"encoding": encoding})),
            );
        }
        cow.into_owned()
    };

    let mut rdr = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(Cursor::new(text.as_bytes()));

    // 解析表头
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| LoadError::InvalidFormat(format!("CSV 表头解析失败: {e}")))?
        .iter()
        .map(|h| h.to_string())
        .collect();

    if headers.is_empty() {
        return Err(LoadError::InvalidFormat("CSV 文件为空（无表头行）".into()));
    }

    let col_map = build_column_map(&headers)?;

    // 校验 6 必填列 — 复用公共函数
    validate_columns(&col_map)?;

    // 逐行解析
    let mut entries = Vec::new();
    for (row_idx, result) in rdr.records().enumerate() {
        let record = result
            .map_err(|e| LoadError::InvalidFormat(format!("第 {} 行解析失败: {e}", row_idx + 2)))?;

        // 跳过全空行
        if record.iter().all(|f| f.trim().is_empty()) {
            continue;
        }

        let entry = parse_csv_record(&record, &col_map);
        if entry.should_skip() {
            diag.warn(
                "reader",
                format!("第 {} 行 word 为空，已跳过", row_idx + 2),
                Some(serde_json::json!({"row": row_idx + 2})),
            );
            continue;
        }
        entries.push(entry);
    }

    Ok(entries)
}

/// 从 CSV 记录按列映射解析为 WordEntry
fn parse_csv_record(record: &csv::StringRecord, col_map: &HashMap<String, usize>) -> WordEntry {
    let cell = |header: &str| -> String {
        col_map
            .get(header)
            .and_then(|&idx| record.get(idx))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    WordEntry {
        word: cell("英文单词"),
        phonetic: cell("英文音标"),
        morphology: cell("词根词缀"),
        example: cell("例句"),
        example_definition: cell("例句释义"),
        definition: cell("单词释义"),
    }
}

/// 提取表头行：将第一行每个单元格转为 String
fn extract_headers(range: &calamine::Range<Data>) -> Result<Vec<String>, LoadError> {
    range
        .rows()
        .next()
        .ok_or_else(|| LoadError::ExcelError("Excel 文件为空（无表头行）".into()))?
        .iter()
        .map(|cell| {
            let s = cell.to_string();
            Ok(s)
        })
        .collect::<Result<Vec<_>, LoadError>>()
}

/// 建立 表头名称 → 列索引 映射
fn build_column_map(headers: &[String]) -> Result<HashMap<String, usize>, LoadError> {
    let mut map = HashMap::new();
    for (idx, header) in headers.iter().enumerate() {
        let trimmed = header.trim();
        if !trimmed.is_empty() {
            map.insert(trimmed.to_string(), idx);
        }
    }
    Ok(map)
}

/// 将一行单元格按列映射解析为 WordEntry
fn parse_row(row: &[Data], col_map: &HashMap<String, usize>) -> WordEntry {
    let cell = |header: &str| -> String {
        col_map
            .get(header)
            .and_then(|&idx| row.get(idx))
            .map(|d| d.to_string().trim().to_string())
            .unwrap_or_default()
    };

    WordEntry {
        word: cell("英文单词"),
        phonetic: cell("英文音标"),
        morphology: cell("词根词缀"),
        example: cell("例句"),
        example_definition: cell("例句释义"),
        definition: cell("单词释义"),
    }
}
