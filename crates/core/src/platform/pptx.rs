//! `pptx::Package`：ZIP 与 OOXML 结构手术的唯一入口（深模块）。
//!
//! 对外只暴露四件事：打开 / 扫描 / 填充 / 落盘。
//! OOXML 的一切细节（`[Content_Types].xml`、`ppt/presentation.xml`、rels、
//! 复制末页的结构修补）都藏在实现里；调用方永远不碰 zip 与 XML。
//!
//! 不变量（见 `docs/design.md` §6.1）：
//! - `open` 失败必给具体原因（不是 ZIP / 缺结构 / 无幻灯片）；
//! - `fill` 页数守恒（I5：词条数 > 页数时复制末页，词条数 < 页数时删除尾部多余页）；
//! - `save_atomic` 落盘后重开自检；任何失败不产出残损包。
//!
//! 实现基准：XML 一律走 `quick-xml` 的事件流（不做字节扫描、不用 `rfind`），
//! 结构修补按元素与属性的语义做——命名空间前缀、已有 ID 都从源文档读出来。

use std::io::{Cursor, Read, Write};
use std::ops::ControlFlow;
use std::path::Path;

use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde_json::json;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::diag::{field, Diag, Target};
use crate::domain::plan::{Artifact, OutputError, OutputPlan};
use crate::domain::schema::{Field, WordEntry, FIELDS};
use crate::ports::{OpObserver, Progress};

/// 内嵌版式资产（由 [`crate::domain::schema::FIELDS`] 的标记生成）。
pub const EMBEDDED_TEMPLATE: &[u8] = include_bytes!("../../assets/template.pptx");

/// 模板里的字段标记体系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScheme {
    /// 内嵌版式：`__WORD__` 等 ASCII 标记（用户不可见）。
    Markers,
    /// 用户模板：`{{单词}}` 等中文占位符（兼容面）。
    Placeholders,
}

impl TokenScheme {
    /// 该方案下某个字段的标记字面量（词表只有 [`FIELDS`] 一处）。
    fn token(self, field: Field) -> &'static str {
        match self {
            TokenScheme::Markers => field.marker(),
            TokenScheme::Placeholders => field.placeholder(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PptxError {
    #[error("不是有效的 PPTX（ZIP 解析失败）: {0}")]
    NotZip(String),
    #[error("PPTX 缺少必需结构: {0}")]
    MissingStructure(&'static str),
    #[error("演示文稿没有任何幻灯片")]
    NoSlides,
    #[error("XML 处理失败: {0}")]
    Xml(String),
    #[error("ZIP 处理失败: {0}")]
    Zip(String),
    #[error(transparent)]
    Output(#[from] OutputError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// ── OOXML 常量（只在本模块出现）──────────────────────────────────────────

/// 内容类型表部件。
const CONTENT_TYPES: &str = "[Content_Types].xml";
/// 演示文稿主体部件。
const PRESENTATION: &str = "ppt/presentation.xml";
/// 演示文稿关系表部件。
const PRESENTATION_RELS: &str = "ppt/_rels/presentation.xml.rels";
/// 幻灯片内容类型。
const CT_SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
/// 关系表内容类型。
const CT_RELS: &str = "application/vnd.openxmlformats-package.relationships+xml";
/// 幻灯片关系类型。
const REL_SLIDE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
/// relationships 命名空间（幻灯片关系属性的归属）。
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// 结构缺失的载荷文案（[`PptxError::MissingStructure`] 是静态串）。
const MISSING_TYPES_ROOT: &str = "[Content_Types].xml 不是内容类型表";
const MISSING_SLD_ID_LIST: &str = "ppt/presentation.xml 的 sldIdLst 里没有 sldId";
const MISSING_SLD_ID_END: &str = "ppt/presentation.xml 缺少 sldIdLst 结束标签";
const MISSING_SLIDE_PART: &str = "删页后找不到保留的幻灯片部件";
const MISSING_RELS_ROOT: &str = "ppt/_rels/presentation.xml.rels 缺少 Relationships";
const MISSING_REL_NS: &str = "ppt/presentation.xml 未声明 relationships 命名空间";

/// 包内一个条目（名称 + 原始字节，顺序即写回顺序）。
#[derive(Debug)]
struct Part {
    name: String,
    data: Vec<u8>,
}

/// 一张幻灯片：序号（`slideN.xml` 的 N）、条目下标、当前 `<a:t>` 文本。
#[derive(Debug)]
struct Slide {
    number: u32,
    part: usize,
    text: String,
}

/// 打开后的 PPTX 包。
#[derive(Debug)]
pub struct Package {
    /// 全部 ZIP 条目（幻灯片 XML 之外的部件原样持有）。
    parts: Vec<Part>,
    /// 幻灯片清单，按 [`Slide::number`] 升序（= 填充顺序）。
    slides: Vec<Slide>,
}

impl Package {
    /// 打开文件。
    pub fn open(path: &Path, diag: &Diag) -> Result<Package, PptxError> {
        let bytes = std::fs::read(path).map_err(|error| {
            report(
                diag,
                "open",
                format!("读取文件失败: {error}"),
                Some(path.display().to_string()),
            );
            PptxError::Io(error)
        })?;
        Self::from_bytes(&bytes, Some(path), diag)
    }

    /// 从字节打开（内嵌资产 / 测试 fixture）。
    pub fn open_bytes(bytes: &[u8], diag: &Diag) -> Result<Package, PptxError> {
        Self::from_bytes(bytes, None, diag)
    }

    /// 幻灯片页数。
    pub fn slide_count(&self) -> usize {
        self.slides.len()
    }

    /// 扫描包中出现的字段标记（按 [`TokenScheme`] 选择的词表）。
    ///
    /// 每张幻灯片的所有 `<a:t>` 文本先拼接再匹配 —— 被 PowerPoint 拆进多个 run 的标记
    /// 也能发现；结果去重并按 [`Field::ALL`] 顺序返回（调用方不必再排序）。
    pub fn scan_tokens(&self, scheme: TokenScheme) -> Vec<Field> {
        Field::ALL
            .into_iter()
            .filter(|field| {
                let token = scheme.token(*field);
                self.slides.iter().any(|slide| slide.text.contains(token))
            })
            .collect()
    }

    /// 逐页填充：按 [`crate::domain::schema::FIELDS`] 替换该页字段文本；
    /// 页数不足时复制最后一页并修补 `[Content_Types].xml` / `presentation.xml` / rels。
    ///
    /// 返回 [`ControlFlow::Break`] 表示观察者请求取消（尚未落盘，无需清理）。
    ///
    /// 页数规则（I5 页数守恒）：词条数 > 版式页数 → 复制末页补足；
    /// 词条数 < 版式页数 → 删除尾部多余页（四处结构一致移除，见 [`Package::truncate_to`]）。
    pub fn fill(
        &mut self,
        entries: &[WordEntry],
        scheme: TokenScheme,
        observer: &mut dyn OpObserver,
        diag: &Diag,
    ) -> Result<ControlFlow<()>, PptxError> {
        if self.slides.is_empty() {
            return Err(PptxError::NoSlides);
        }
        if entries.is_empty() {
            report_warn(
                diag,
                "fill",
                "没有词条可填充，版式原样返回".to_string(),
                None,
            );
            return Ok(ControlFlow::Continue(()));
        }

        let template_slides = self.slides.len();
        let total = entries.len();
        if total > template_slides {
            diag.info(
                Target::Pptx,
                format!(
                    "词条数 {total} 超过版式页数 {template_slides}，复制最后一页补足 {} 页",
                    total - template_slides
                ),
                Some(json!({
                    field::OP: "fill",
                    field::ENTRIES: total,
                    field::SLIDES: template_slides,
                })),
            );
        }

        // 复制源 = 末页**未填充**的原件（末页随后会被就地填充）。
        let last = self.slides[template_slides - 1].number;
        let source_part = slide_part_name(last);
        let source_xml = self.parts[self.slides[template_slides - 1].part].data.clone();
        let source_rels = self
            .part(&slide_rels_part_name(last))
            .map(|part| part.data.clone());
        if total > template_slides && source_rels.is_none() {
            report_warn(
                diag,
                "fill",
                format!(
                    "版式末页没有 rels（{}），复制页将不带 rels",
                    slide_rels_part_name(last)
                ),
                Some(slide_part_name(last)),
            );
        }

        for (index, entry) in entries.iter().enumerate() {
            // 替换值保持原样（不在这里转义），写回时对合并后的文本统一转义一次。
            let replacements: Vec<(&'static str, &str)> = FIELDS
                .iter()
                .map(|meta| (scheme.token(meta.field), entry.value(meta.field)))
                .collect();

            if index < template_slides {
                let part = self.slides[index].part;
                let name = self.parts[part].name.clone();
                let (data, text) = fill_slide(&self.parts[part].data, &replacements, &name)
                    .map_err(|error| {
                        report(
                            diag,
                            "fill",
                            format!("填充第 {} 页失败: {error}", index + 1),
                            Some(name.clone()),
                        );
                        error
                    })?;
                self.parts[part].data = data;
                self.slides[index].text = text;
            } else {
                let number = self.next_free_number()?;
                let (data, text) = fill_slide(&source_xml, &replacements, &source_part).map_err(
                    |error| {
                        report(
                            diag,
                            "fill",
                            format!("复制末页填充第 {} 页失败: {error}", index + 1),
                            Some(source_part.clone()),
                        );
                        error
                    },
                )?;
                self.append_slide(number, data, text, source_rels.as_deref(), diag)?;
            }

            diag.info(
                Target::Pptx,
                format!("填充第 {}/{} 页", index + 1, total),
                Some(json!({
                    field::OP: "fill",
                    field::SLIDE: index + 1,
                    field::SLIDES: total,
                })),
            );
            if observer.on(&Progress::Slides {
                done: index + 1,
                total,
            }) == ControlFlow::Break(())
            {
                return Ok(ControlFlow::Break(()));
            }
        }
        Ok(ControlFlow::Continue(()))
    }

    /// 原子落盘 + 落盘后重开自检（页数守恒）。
    pub fn save_atomic(&self, plan: &OutputPlan, diag: &Diag) -> Result<Artifact, PptxError> {
        let path = plan.resolve().map_err(|error| {
            report(
                diag,
                "save",
                format!("输出落点不可用: {error}"),
                Some(plan.dir.join(&plan.name).display().to_string()),
            );
            PptxError::Output(error)
        })?;
        let expected = self.slide_count();
        let bytes = self.to_zip(diag)?;
        crate::platform::io::write_atomic(&path, &bytes, diag)?;

        // 自检（I6）：重开刚落盘的包；页数与填充后不一致就删掉，绝不留下残损产物。
        match Package::open(&path, diag) {
            Ok(reopened) if reopened.slide_count() == expected => {}
            Ok(reopened) => {
                let error = PptxError::Xml(format!(
                    "落盘自检失败：期望 {expected} 页，实际 {} 页",
                    reopened.slide_count()
                ));
                report(
                    diag,
                    "save",
                    error.to_string(),
                    Some(path.display().to_string()),
                );
                crate::platform::io::remove_file(&path, diag);
                return Err(error);
            }
            Err(error) => {
                crate::platform::io::remove_file(&path, diag);
                return Err(error);
            }
        }

        diag.info(
            Target::Pptx,
            format!("已落盘：{}（{expected} 页）", path.display()),
            Some(json!({
                field::OP: "save",
                field::PATH: path.display().to_string(),
                field::SLIDES: expected,
            })),
        );
        Ok(Artifact {
            path,
            slides: expected,
        })
    }
}

// ── 打开 ─────────────────────────────────────────────────────────────────

impl Package {
    fn from_bytes(bytes: &[u8], origin: Option<&Path>, diag: &Diag) -> Result<Package, PptxError> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
            report(
                diag,
                "open",
                format!("不是有效的 PPTX: {error}"),
                origin.map(|path| path.display().to_string()),
            );
            PptxError::NotZip(error.to_string())
        })?;

        let mut parts = Vec::with_capacity(archive.len());
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| {
                report(
                    diag,
                    "open",
                    format!("读取 ZIP 条目 {index} 失败: {error}"),
                    origin.map(|path| path.display().to_string()),
                );
                PptxError::Zip(error.to_string())
            })?;
            let name = entry.name().to_string();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).map_err(|error| {
                report(
                    diag,
                    "open",
                    format!("解压 {name} 失败: {error}"),
                    Some(name.clone()),
                );
                PptxError::Zip(error.to_string())
            })?;
            parts.push(Part { name, data });
        }
        Self::assemble(parts, origin, diag)
    }

    /// 校验结构并枚举幻灯片（构造 [`Package`] 的唯一路径）。
    fn assemble(parts: Vec<Part>, origin: Option<&Path>, diag: &Diag) -> Result<Package, PptxError> {
        let origin_name = || origin.map(|path| path.display().to_string());
        for required in [CONTENT_TYPES, PRESENTATION] {
            if !parts.iter().any(|part| part.name == required) {
                report(
                    diag,
                    "open",
                    format!("PPTX 缺少必需部件: {required}"),
                    origin_name(),
                );
                return Err(PptxError::MissingStructure(required));
            }
        }

        // 枚举 ppt/slides/slideN.xml 并按 N 升序；同名重复部件保留最后一份（ZIP 读取语义）。
        let mut numbered: Vec<(u32, usize)> = parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| slide_number(&part.name).map(|number| (number, index)))
            .collect();
        numbered.sort_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)));

        let mut slides = Vec::with_capacity(numbered.len());
        for (number, part) in numbered {
            if slides.last().map(|slide: &Slide| slide.number) == Some(number) {
                continue;
            }
            let name = parts[part].name.clone();
            let text = slide_text(&parts[part].data, &name).map_err(|error| {
                let message = format!("幻灯片 XML 无法解析: {error}");
                report(diag, "open", message, Some(name.clone()));
                error
            })?;
            slides.push(Slide {
                number,
                part,
                text,
            });
        }

        if slides.is_empty() {
            report(
                diag,
                "open",
                "演示文稿没有任何幻灯片".to_string(),
                origin_name(),
            );
            return Err(PptxError::NoSlides);
        }
        Ok(Package { parts, slides })
    }

    fn part(&self, name: &str) -> Option<&Part> {
        self.parts.iter().find(|part| part.name == name)
    }

    /// 下一个空闲的幻灯片序号（排在已有序号之后，并避开一切已占用的部件名）。
    fn next_free_number(&self) -> Result<u32, PptxError> {
        let mut number = self
            .slides
            .iter()
            .map(|slide| slide.number)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| xml_error("ppt/slides", "幻灯片序号已用尽"))?;
        while self
            .parts
            .iter()
            .any(|part| part.name == slide_part_name(number))
        {
            number = number
                .checked_add(1)
                .ok_or_else(|| xml_error("ppt/slides", "幻灯片序号已用尽"))?;
        }
        Ok(number)
    }
}

// ── 结构修补（复制末页）──────────────────────────────────────────────────

impl Package {
    /// 追加一页复制出来的幻灯片，并修补三处结构（sldId / 关系 ID 三处一致且不重复）。
    fn append_slide(
        &mut self,
        number: u32,
        data: Vec<u8>,
        text: String,
        rels: Option<&[u8]>,
        diag: &Diag,
    ) -> Result<(), PptxError> {
        let slide_name = slide_part_name(number);
        let rels_name = rels.map(|_| slide_rels_part_name(number));

        // 先把三份修补结果算完：任一失败都不留下半套结构。
        let content_types = self
            .part(CONTENT_TYPES)
            .ok_or(PptxError::MissingStructure(CONTENT_TYPES))?;
        let patched_types =
            patch_content_types(&content_types.data, &slide_name, rels_name.as_deref()).map_err(
                |error| {
                    let message = format!("修补内容类型表失败: {error}");
                    report(diag, "fill", message, Some(slide_name.clone()));
                    error
                },
            )?;
        let presentation = self
            .part(PRESENTATION)
            .ok_or(PptxError::MissingStructure(PRESENTATION))?;
        let presentation_rels = self
            .part(PRESENTATION_RELS)
            .ok_or(PptxError::MissingStructure(PRESENTATION_RELS))?;
        let (patched_rels, rel_id) = add_slide_relationship(&presentation_rels.data, &slide_name)
            .map_err(|error| {
                let message = format!("追加幻灯片关系失败: {error}");
                report(diag, "fill", message, Some(PRESENTATION_RELS.to_string()));
                error
            })?;
        let (patched_presentation, slide_id) =
            add_sld_id(&presentation.data, &rel_id).map_err(|error| {
                let message = format!("追加 sldId 失败: {error}");
                report(diag, "fill", message, Some(PRESENTATION.to_string()));
                error
            })?;

        if let Some(part) = self.parts.iter_mut().find(|part| part.name == CONTENT_TYPES) {
            part.data = patched_types;
        }
        if let Some(part) = self
            .parts
            .iter_mut()
            .find(|part| part.name == PRESENTATION_RELS)
        {
            part.data = patched_rels;
        }
        if let Some(part) = self.parts.iter_mut().find(|part| part.name == PRESENTATION) {
            part.data = patched_presentation;
        }
        if let (Some(name), Some(rels)) = (rels_name, rels) {
            self.parts.push(Part {
                name,
                data: rels.to_vec(),
            });
        }
        self.parts.push(Part {
            name: slide_name.clone(),
            data,
        });
        let index = self.parts.len() - 1;
        self.slides.push(Slide {
            number,
            part: index,
            text,
        });

        diag.info(
            Target::Pptx,
            format!("复制末页 → {slide_name}（sldId {slide_id}，{rel_id}）"),
            Some(json!({
                field::OP: "fill",
                field::NAME: slide_name,
                field::SLIDE: number,
            })),
        );
        Ok(())
    }

    /// 序列化整包：全部条目按原顺序写回（新增部件已在末尾）。
    fn to_zip(&self, diag: &Diag) -> Result<Vec<u8>, PptxError> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            for part in &self.parts {
                writer.start_file(part.name.as_str(), options).map_err(|error| {
                    zip_error(diag, format!("写入 ZIP 条目 {} 失败", part.name), error)
                })?;
                writer.write_all(&part.data).map_err(|error| {
                    zip_error(diag, format!("写入 ZIP 数据 {} 失败", part.name), error)
                })?;
            }
            writer
                .finish()
                .map_err(|error| zip_error(diag, "完成 ZIP 失败".to_string(), error))?;
        }
        Ok(buffer.into_inner())
    }
}

/// 结构化修补 `[Content_Types].xml`：为新幻灯片（及其 rels）补 Override 项。
fn patch_content_types(
    xml: &[u8],
    slide_part: &str,
    rels_part: Option<&str>,
) -> Result<Vec<u8>, PptxError> {
    let mut reader = Reader::from_reader(xml);
    reader.trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(xml.len() + 256));
    let mut buf = Vec::new();
    let mut declared: Vec<String> = Vec::new();
    let mut injected = false;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|error| xml_error(CONTENT_TYPES, error))?;
        if let Event::Eof = event {
            break;
        }
        if let Event::Start(start) | Event::Empty(start) = &event {
            if local_name(start.name().as_ref()) == b"Override" {
                if let Some(part_name) = attribute_value(start, "PartName", CONTENT_TYPES)? {
                    declared.push(part_name);
                }
            }
        }
        if let Event::End(end) = &event {
            if local_name(end.name().as_ref()) == b"Types" {
                if injected {
                    return Err(xml_error(CONTENT_TYPES, "</Types> 出现多次"));
                }
                let prefix = String::from_utf8_lossy(prefix_of(end.name().as_ref())).into_owned();
                let mut additions: Vec<(&str, &str)> = vec![(slide_part, CT_SLIDE)];
                if let Some(rels_part) = rels_part {
                    additions.push((rels_part, CT_RELS));
                }
                for (part, content_type) in additions {
                    let part_name = format!("/{part}");
                    if declared.iter().any(|name| *name == part_name) {
                        continue;
                    }
                    let mut element = BytesStart::new(format!("{prefix}Override"));
                    element.push_attribute(("PartName", escape_owned(&part_name).as_str()));
                    element.push_attribute(("ContentType", content_type));
                    writer
                        .write_event(Event::Empty(element))
                        .map_err(|error| xml_error(CONTENT_TYPES, error))?;
                }
                injected = true;
            }
        }
        writer
            .write_event(&event)
            .map_err(|error| xml_error(CONTENT_TYPES, error))?;
    }

    if !injected {
        return Err(PptxError::MissingStructure(MISSING_TYPES_ROOT));
    }
    Ok(writer.into_inner())
}

/// 在 `ppt/_rels/presentation.xml.rels` 追加幻灯片关系；返回（新 XML，使用的 rId）。
fn add_slide_relationship(xml: &[u8], slide_part: &str) -> Result<(Vec<u8>, String), PptxError> {
    // 关系 Target 相对 rels 宿主目录（`ppt/`）。
    let target = slide_part.strip_prefix("ppt/").unwrap_or(slide_part);
    let mut reader = Reader::from_reader(xml);
    reader.trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(xml.len() + 256));
    let mut buf = Vec::new();
    let mut max_id = 0u32;
    let mut exemplar: Option<BytesStart<'static>> = None;
    let mut added: Option<String> = None;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|error| xml_error(PRESENTATION_RELS, error))?;
        if let Event::Eof = event {
            break;
        }
        if let Event::Start(start) | Event::Empty(start) = &event {
            if local_name(start.name().as_ref()) == b"Relationship" {
                if let Some(id) = attribute_value(start, "Id", PRESENTATION_RELS)? {
                    if let Some(number) = id
                        .strip_prefix("rId")
                        .and_then(|digits| digits.parse::<u32>().ok())
                    {
                        max_id = max_id.max(number);
                    }
                }
                if exemplar.is_none() {
                    if let Some(kind) = attribute_value(start, "Type", PRESENTATION_RELS)? {
                        if kind == REL_SLIDE {
                            exemplar = Some(owned_element(start, PRESENTATION_RELS)?);
                        }
                    }
                }
            }
        }
        if let Event::End(end) = &event {
            if local_name(end.name().as_ref()) == b"Relationships" {
                if added.is_some() {
                    return Err(xml_error(PRESENTATION_RELS, "</Relationships> 出现多次"));
                }
                let number = max_id
                    .checked_add(1)
                    .ok_or_else(|| xml_error(PRESENTATION_RELS, "rId 序号已用尽"))?;
                let rel_id = format!("rId{number}");
                let prefix = String::from_utf8_lossy(prefix_of(end.name().as_ref())).into_owned();
                let element = match &exemplar {
                    Some(source) => cloned_element(
                        source,
                        &[("Id", &rel_id), ("Type", REL_SLIDE), ("Target", target)],
                    )?,
                    None => fresh_element(
                        &format!("{prefix}Relationship"),
                        &[("Id", &rel_id), ("Type", REL_SLIDE), ("Target", target)],
                    ),
                };
                writer
                    .write_event(Event::Empty(element))
                    .map_err(|error| xml_error(PRESENTATION_RELS, error))?;
                added = Some(rel_id);
            }
        }
        writer
            .write_event(&event)
            .map_err(|error| xml_error(PRESENTATION_RELS, error))?;
    }

    match added {
        Some(rel_id) => Ok((writer.into_inner(), rel_id)),
        None => Err(PptxError::MissingStructure(MISSING_RELS_ROOT)),
    }
}

/// 在 `ppt/presentation.xml` 的 `sldIdLst` 末尾追加一条 `sldId`；返回（新 XML，新 sldId）。
fn add_sld_id(xml: &[u8], rel_id: &str) -> Result<(Vec<u8>, u32), PptxError> {
    let mut reader = Reader::from_reader(xml);
    reader.trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(xml.len() + 256));
    let mut buf = Vec::new();
    let mut namespaces: Vec<(String, String)> = Vec::new();
    let mut in_list = false;
    let mut max_id: Option<u32> = None;
    let mut exemplar: Option<BytesStart<'static>> = None;
    let mut rel_key: Option<String> = None;
    let mut added: Option<u32> = None;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|error| xml_error(PRESENTATION, error))?;
        if let Event::Eof = event {
            break;
        }
        if let Event::Start(start) | Event::Empty(start) = &event {
                collect_namespaces(start, &mut namespaces, PRESENTATION)?;
                let name = start.name();
                let local = local_name(name.as_ref());
            if local == b"sldIdLst" {
                in_list = true;
            } else if in_list && local == b"sldId" {
                if let Some(id) = attribute_value(start, "id", PRESENTATION)? {
                    if let Ok(number) = id.parse::<u32>() {
                        max_id = Some(max_id.map_or(number, |current| current.max(number)));
                    }
                }
                if exemplar.is_none() {
                    rel_key = namespaced_key(start, &namespaces, REL_NS, "id", PRESENTATION)?;
                    exemplar = Some(owned_element(start, PRESENTATION)?);
                }
            }
        }
        if let Event::End(end) = &event {
            if local_name(end.name().as_ref()) == b"sldIdLst" {
                if added.is_some() {
                    return Err(xml_error(PRESENTATION, "</p:sldIdLst> 出现多次"));
                }
                let source = exemplar
                    .as_ref()
                    .ok_or(PptxError::MissingStructure(MISSING_SLD_ID_LIST))?;
                let slide_id = match max_id {
                    Some(current) => current
                        .checked_add(1)
                        .ok_or_else(|| xml_error(PRESENTATION, "sldId 序号已用尽"))?,
                    None => 256,
                };
                let key = match rel_key.clone() {
                    Some(key) => key,
                    None => match namespaces.iter().find(|(_, ns)| ns == REL_NS) {
                        Some((prefix, _)) => format!("{prefix}:id"),
                        None => return Err(PptxError::MissingStructure(MISSING_REL_NS)),
                    },
                };
                let slide_id_text = slide_id.to_string();
                let element = cloned_element(
                    source,
                    &[("id", slide_id_text.as_str()), (key.as_str(), rel_id)],
                )?;
                writer
                    .write_event(Event::Empty(element))
                    .map_err(|error| xml_error(PRESENTATION, error))?;
                added = Some(slide_id);
                in_list = false;
            }
        }
        writer
            .write_event(&event)
            .map_err(|error| xml_error(PRESENTATION, error))?;
    }

    match added {
        Some(slide_id) => Ok((writer.into_inner(), slide_id)),
        None if exemplar.is_none() => Err(PptxError::MissingStructure(MISSING_SLD_ID_LIST)),
        None => Err(PptxError::MissingStructure(MISSING_SLD_ID_END)),
    }
}

// ── 幻灯片文本：读取与替换 ───────────────────────────────────────────────

/// 一页幻灯片里所有 `<a:t>` 文本的拼接（扫描用；跨 run 的标记在这里才成词）。
fn slide_text(xml: &[u8], part: &str) -> Result<String, PptxError> {
    let mut reader = Reader::from_reader(xml);
    reader.trim_text(false);
    let mut buf = Vec::new();
    let mut text = String::new();
    let mut collecting = false;

    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|error| xml_error(part, error))?
        {
            Event::Eof => break,
            Event::Start(start) if is_drawing(start.name().as_ref(), b"t") => collecting = true,
            Event::End(end) if is_drawing(end.name().as_ref(), b"t") => collecting = false,
            Event::Text(chunk) if collecting => {
                text.push_str(&chunk.unescape().map_err(|error| xml_error(part, error))?);
            }
            Event::CData(chunk) if collecting => {
                text.push_str(&String::from_utf8_lossy(&chunk));
            }
            _ => {}
        }
    }
    Ok(text)
}

/// 替换一页幻灯片里的字段标记；返回（新 XML，新 XML 的 `<a:t>` 文本）。
///
/// 段落内所有常规 run 的 `<a:t>` 文本先合并 → 替换 → 写回本段第一个 run，其余 run 清空：
/// PowerPoint 会把 `{{单词}}` 拆进多个 run，合并是替换到它的唯一办法（首个 run 的样式保留）。
/// `<a:fld>`（页码等自动字段）里的文本不参与合并，原样保留。
fn fill_slide(
    xml: &[u8],
    replacements: &[(&str, &str)],
    part: &str,
) -> Result<(Vec<u8>, String), PptxError> {
    let mut reader = Reader::from_reader(xml);
    reader.trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(xml.len() + 256));
    let mut buf = Vec::new();
    let mut paragraph: Option<Vec<Event<'static>>> = None;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|error| xml_error(part, error))?;
        if let Event::Eof = event {
            break;
        }
        if let Some(buffered) = paragraph.as_mut() {
            let owned = event.into_owned();
            let closed = matches!(&owned, Event::End(end) if is_drawing(end.name().as_ref(), b"p"));
            buffered.push(owned);
            if closed {
                let events = paragraph.take().expect("段落缓冲存在");
                write_paragraph(&events, replacements, &mut writer, part)?;
            }
            continue;
        }
        let paragraph_start =
            matches!(&event, Event::Start(start) if is_drawing(start.name().as_ref(), b"p"));
        if paragraph_start {
            paragraph = Some(vec![event.into_owned()]);
            continue;
        }
        writer
            .write_event(&event)
            .map_err(|error| xml_error(part, error))?;
    }

    if paragraph.is_some() {
        return Err(xml_error(part, "段落未闭合"));
    }
    let data = writer.into_inner();
    let text = slide_text(&data, part)?;
    Ok((data, text))
}

/// 写回一个段落：合并文本 → 替换 → 首个常规 run 承载全文，其余 run 清空。
fn write_paragraph(
    events: &[Event<'static>],
    replacements: &[(&str, &str)],
    writer: &mut Writer<Vec<u8>>,
    part: &str,
) -> Result<(), PptxError> {
    // 段落里的 `<a:t>` 区域（跳过 `<a:fld>` 内的自动字段文本）。
    let mut regions: Vec<(usize, usize)> = Vec::new();
    let mut field_depth = 0usize;
    let mut open: Option<usize> = None;
    for (index, event) in events.iter().enumerate() {
        match event {
            Event::Start(element) => {
                let name = element.name();
                let name = name.as_ref();
                if is_drawing(name, b"t") && field_depth == 0 {
                    open = Some(index);
                } else if is_drawing(name, b"fld") {
                    field_depth += 1;
                }
            }
            Event::End(element) => {
                let name = element.name();
                let name = name.as_ref();
                if is_drawing(name, b"t") {
                    if let Some(start) = open.take() {
                        regions.push((start, index));
                    }
                } else if is_drawing(name, b"fld") {
                    field_depth = field_depth.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    if regions.is_empty() {
        for event in events {
            writer
                .write_event(event)
                .map_err(|error| xml_error(part, error))?;
        }
        return Ok(());
    }

    let mut merged = String::new();
    for &(start, end) in &regions {
        for event in &events[start + 1..end] {
            match event {
                Event::Text(chunk) => {
                    merged.push_str(&chunk.unescape().map_err(|error| xml_error(part, error))?);
                }
                Event::CData(chunk) => merged.push_str(&String::from_utf8_lossy(chunk)),
                _ => {}
            }
        }
    }
    for (token, value) in replacements {
        if merged.contains(token) {
            merged = merged.replace(token, value);
        }
    }
    let escaped = quick_xml::escape::escape(&merged);

    let mut region_index = 0usize;
    let mut index = 0usize;
    let mut first = true;
    while index < events.len() {
        match regions.get(region_index).copied() {
            Some((start, end)) if start == index => {
                region_index += 1;
                writer
                    .write_event(&events[start])
                    .map_err(|error| xml_error(part, error))?;
                if first {
                    if !escaped.is_empty() {
                        writer
                            .write_event(Event::Text(BytesText::from_escaped(escaped.as_ref())))
                            .map_err(|error| xml_error(part, error))?;
                    }
                    first = false;
                }
                writer
                    .write_event(&events[end])
                    .map_err(|error| xml_error(part, error))?;
                index = end + 1;
            }
            _ => {
                writer
                    .write_event(&events[index])
                    .map_err(|error| xml_error(part, error))?;
                index += 1;
            }
        }
    }
    Ok(())
}

// ── 小工具 ───────────────────────────────────────────────────────────────

/// `ppt/slides/slideN.xml` → `Some(N)`。
fn slide_number(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok()
}

fn slide_part_name(number: u32) -> String {
    format!("ppt/slides/slide{number}.xml")
}

fn slide_rels_part_name(number: u32) -> String {
    format!("ppt/slides/_rels/slide{number}.xml.rels")
}

/// 去掉命名空间前缀（`a:t` → `t`）。
fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().position(|&byte| byte == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    }
}

/// 命名空间前缀（`a:t` → `a`；`t` → 空）。
fn prefix_of(name: &[u8]) -> &[u8] {
    match name.iter().position(|&byte| byte == b':') {
        Some(index) => &name[..index],
        None => &[],
    }
}

/// 是否 DrawingML 元素（无前缀或 `a:` 前缀，且本地名等于 `local`）。
fn is_drawing(name: &[u8], local: &[u8]) -> bool {
    if local_name(name) != local {
        return false;
    }
    let prefix = prefix_of(name);
    prefix.is_empty() || prefix == b"a"
}

/// 元素属性值（按精确键；已反转义）。
fn attribute_value(element: &BytesStart, key: &str, part: &str) -> Result<Option<String>, PptxError> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| xml_error(part, error))?;
        if attribute.key.as_ref() == key.as_bytes() {
            return decode_value(&attribute.value, part).map(Some);
        }
    }
    Ok(None)
}

/// 元素里属于命名空间 `namespace`、本地名为 `local` 的属性全键（如 `r:id`）。
fn namespaced_key(
    element: &BytesStart,
    namespaces: &[(String, String)],
    namespace: &str,
    local: &str,
    part: &str,
) -> Result<Option<String>, PptxError> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| xml_error(part, error))?;
        let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        if local_name(key.as_bytes()) != local.as_bytes() {
            continue;
        }
        let prefix = match key.split_once(':') {
            Some((prefix, _)) => prefix,
            None => continue,
        };
        if namespaces
            .iter()
            .any(|(declared, ns)| declared == prefix && ns == namespace)
        {
            return Ok(Some(key));
        }
    }
    Ok(None)
}

/// 收集元素上的 `xmlns:*` 声明（默认命名空间不适用于属性，故忽略）。
fn collect_namespaces(
    element: &BytesStart,
    namespaces: &mut Vec<(String, String)>,
    part: &str,
) -> Result<(), PptxError> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| xml_error(part, error))?;
        let prefix = match attribute.key.as_ref().strip_prefix(b"xmlns:") {
            Some(prefix) => String::from_utf8_lossy(prefix).into_owned(),
            None => continue,
        };
        let namespace = decode_value(&attribute.value, part)?;
        if !namespaces.iter().any(|(declared, _)| *declared == prefix) {
            namespaces.push((prefix, namespace));
        }
    }
    Ok(())
}

/// 元素的不可变拷贝（属性字节原样保留）。
fn owned_element(element: &BytesStart, part: &str) -> Result<BytesStart<'static>, PptxError> {
    let mut owned = BytesStart::new(String::from_utf8_lossy(element.name().as_ref()).into_owned());
    for attribute in element.attributes() {
        owned.push_attribute(attribute.map_err(|error| xml_error(part, error))?);
    }
    Ok(owned)
}

/// 复制元素形态，按精确键替换属性值；源里没有的键补在末尾。
fn cloned_element(
    source: &BytesStart,
    updates: &[(&str, &str)],
) -> Result<BytesStart<'static>, PptxError> {
    let mut element = BytesStart::new(String::from_utf8_lossy(source.name().as_ref()).into_owned());
    let mut applied = vec![false; updates.len()];
    for attribute in source.attributes() {
        let attribute = attribute.map_err(|error| xml_error("pptx", error))?;
        let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        let mut value = String::from_utf8_lossy(&attribute.value).into_owned();
        for (index, (update_key, update_value)) in updates.iter().enumerate() {
            if key == *update_key {
                value = escape_owned(update_value);
                applied[index] = true;
            }
        }
        element.push_attribute((key.as_str(), value.as_str()));
    }
    for (index, (update_key, update_value)) in updates.iter().enumerate() {
        if !applied[index] {
            element.push_attribute((*update_key, escape_owned(update_value).as_str()));
        }
    }
    Ok(element)
}

/// 从头构造一个空元素。
fn fresh_element(name: &str, attributes: &[(&str, &str)]) -> BytesStart<'static> {
    let mut element = BytesStart::new(name.to_string());
    for (key, value) in attributes {
        element.push_attribute((*key, escape_owned(value).as_str()));
    }
    element
}

fn escape_owned(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

/// 属性值解码（UTF-8 + 实体反转义）。
fn decode_value(raw: &[u8], part: &str) -> Result<String, PptxError> {
    let raw = std::str::from_utf8(raw).map_err(|error| xml_error(part, error))?;
    Ok(quick_xml::escape::unescape(raw)
        .map_err(|error| xml_error(part, error))?
        .into_owned())
}

fn xml_error(part: &str, error: impl std::fmt::Display) -> PptxError {
    PptxError::Xml(format!("{part}: {error}"))
}

/// 记一条 ZIP 错误事件并构造 [`PptxError::Zip`]。
fn zip_error(diag: &Diag, message: String, error: impl std::fmt::Display) -> PptxError {
    report(diag, "save", format!("{message}: {error}"), None);
    PptxError::Zip(error.to_string())
}

/// 记一条 pptx 错误事件（发生处 = `op`，部件 = `part`）。
fn report(diag: &Diag, op: &'static str, message: String, part: Option<String>) {
    diag.error(
        Target::Pptx,
        message,
        Some(json!({ field::OP: op, field::NAME: part.unwrap_or_default() })),
    );
}

fn report_warn(diag: &Diag, op: &'static str, message: String, part: Option<String>) {
    diag.warn(
        Target::Pptx,
        message,
        Some(json!({ field::OP: op, field::NAME: part.unwrap_or_default() })),
    );
}
