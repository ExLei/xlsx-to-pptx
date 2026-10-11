//! 字段表：六字段的**表头 / 占位符 / 内部标记 / 标签 / 规范顺序**的唯一来源。
//!
//! 加字段 = 这里加一行 + 重新生成内嵌资产（`crates/core/assets/template.pptx`）。
//! 兼容面（不可改）：表头与 `{{占位符}}` 与旧版一致；内部标记用户不可见。

use std::path::PathBuf;

/// 六个固定字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Word,
    Phonetic,
    Morphology,
    Example,
    ExampleDef,
    Definition,
}

/// 字段的元信息（全部分支合并进一张表，杜绝平行副本）。
#[derive(Debug, Clone, Copy)]
pub struct FieldMeta {
    pub field: Field,
    /// 表格列名（兼容面，不可改）。
    pub header: &'static str,
    /// 用户模板占位符（兼容面，不可改）。
    pub placeholder: &'static str,
    /// 内嵌版式标记（内部专用，用户不可见）。
    pub marker: &'static str,
    /// 内置 PNG 版式的行标签。
    pub label: &'static str,
}

/// 规范顺序 = 表格列顺序 = 模板填充顺序（兼容面）。
pub const FIELDS: [FieldMeta; 6] = [
    FieldMeta {
        field: Field::Word,
        header: "英文单词",
        placeholder: "{{单词}}",
        marker: "__WORD__",
        label: "单词",
    },
    FieldMeta {
        field: Field::Phonetic,
        header: "英文音标",
        placeholder: "{{音标}}",
        marker: "__PHONETIC__",
        label: "音标",
    },
    FieldMeta {
        field: Field::Morphology,
        header: "词根词缀",
        placeholder: "{{词根词缀}}",
        marker: "__MORPH__",
        label: "词根词缀：",
    },
    FieldMeta {
        field: Field::Example,
        header: "例句",
        placeholder: "{{例句}}",
        marker: "__EXAMPLE__",
        label: "例句：",
    },
    FieldMeta {
        field: Field::ExampleDef,
        header: "例句释义",
        placeholder: "{{例句释义}}",
        marker: "__EX_DEF__",
        label: "例句释义：",
    },
    FieldMeta {
        field: Field::Definition,
        header: "单词释义",
        placeholder: "{{单词释义}}",
        marker: "__DEF__",
        label: "单词释义：",
    },
];

impl Field {
    pub const ALL: [Field; 6] = [
        Field::Word,
        Field::Phonetic,
        Field::Morphology,
        Field::Example,
        Field::ExampleDef,
        Field::Definition,
    ];

    pub fn index(self) -> usize {
        match self {
            Field::Word => 0,
            Field::Phonetic => 1,
            Field::Morphology => 2,
            Field::Example => 3,
            Field::ExampleDef => 4,
            Field::Definition => 5,
        }
    }

    pub fn meta(self) -> &'static FieldMeta {
        &FIELDS[self.index()]
    }

    pub fn header(self) -> &'static str {
        self.meta().header
    }

    pub fn placeholder(self) -> &'static str {
        self.meta().placeholder
    }

    pub fn marker(self) -> &'static str {
        self.meta().marker
    }

    pub fn label(self) -> &'static str {
        self.meta().label
    }

    /// 表格列名 → 字段（前后空白忽略）。
    pub fn from_header(header: &str) -> Option<Field> {
        FIELDS
            .iter()
            .find(|m| m.header == header.trim())
            .map(|m| m.field)
    }
}

/// 词条：词汇表的一行。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WordEntry {
    pub word: String,
    pub phonetic: String,
    pub morphology: String,
    pub example: String,
    pub example_definition: String,
    pub definition: String,
}

impl WordEntry {
    /// 按字段取值（表驱动代码的统一访问口）。
    pub fn value(&self, field: Field) -> &str {
        match field {
            Field::Word => &self.word,
            Field::Phonetic => &self.phonetic,
            Field::Morphology => &self.morphology,
            Field::Example => &self.example,
            Field::ExampleDef => &self.example_definition,
            Field::Definition => &self.definition,
        }
    }

    /// 唯一跳过规则：`英文单词` 为空（trim 后）。其余字段为空一律保留空位。
    pub fn is_skippable(&self) -> bool {
        self.word.trim().is_empty()
    }
}

/// 工作表信息（GUI 下拉与 CLI 选表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetInfo {
    pub name: String,
}

/// 输入源类别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// Excel：`sheet = None` 时取第一个工作表。
    Excel { sheet: Option<String> },
    /// CSV：编码标签（`UTF-8` / `GBK` / `GB2312` / `GB18030`）。
    Csv { encoding: String },
}

/// 输入源：路径 + 类别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub kind: SourceKind,
}

/// CSV 编码候选（GUI 下拉与校验共用这一份）。
pub const CSV_ENCODINGS: [&str; 4] = ["UTF-8", "GBK", "GB2312", "GB18030"];

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> WordEntry {
        WordEntry {
            word: "apple".into(),
            phonetic: "ˈæp.əl".into(),
            morphology: "ap-ple".into(),
            example: "I ate an apple.".into(),
            example_definition: "我吃了一个苹果。".into(),
            definition: "n. 苹果".into(),
        }
    }

    #[test]
    fn fields_table_matches_canonical_order() {
        assert_eq!(FIELDS.map(|m| m.field), Field::ALL);
    }

    #[test]
    fn header_lookup_roundtrip_and_trim() {
        for m in FIELDS {
            assert_eq!(Field::from_header(m.header), Some(m.field));
            assert_eq!(
                Field::from_header(&format!("  {}  ", m.header)),
                Some(m.field)
            );
        }
        assert_eq!(Field::from_header("不存在"), None);
    }

    #[test]
    fn placeholders_and_markers_are_unique_and_shaped() {
        let mut placeholders = std::collections::HashSet::new();
        let mut markers = std::collections::HashSet::new();
        for m in FIELDS {
            assert!(
                m.placeholder.starts_with("{{") && m.placeholder.ends_with("}}"),
                "{}",
                m.placeholder
            );
            assert!(
                m.marker.starts_with("__") && m.marker.ends_with("__"),
                "{}",
                m.marker
            );
            assert!(
                placeholders.insert(m.placeholder),
                "占位符重复: {}",
                m.placeholder
            );
            assert!(markers.insert(m.marker), "标记重复: {}", m.marker);
        }
    }

    #[test]
    fn skippable_only_when_word_blank() {
        assert!(WordEntry::default().is_skippable());
        let mut e = WordEntry::default();
        e.word = "   ".into();
        assert!(e.is_skippable());
        // 有单词、其余全空 → 不跳过（保留空位）
        let mut e = WordEntry::default();
        e.word = "apple".into();
        assert!(!e.is_skippable());
    }

    #[test]
    fn value_reads_each_field() {
        let e = entry();
        assert_eq!(e.value(Field::Word), "apple");
        assert_eq!(e.value(Field::Phonetic), "ˈæp.əl");
        assert_eq!(e.value(Field::Morphology), "ap-ple");
        assert_eq!(e.value(Field::Example), "I ate an apple.");
        assert_eq!(e.value(Field::ExampleDef), "我吃了一个苹果。");
        assert_eq!(e.value(Field::Definition), "n. 苹果");
    }
}
