//! Strict bounded current-content packages; no database, history, or vector artifacts are accepted.
//! 严格有界当前内容套件；不接受数据库、历史或向量产物。

use super::content::{PackageExportContent, ValidatedPackageContent};
use super::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

/// Exact current manifest entry name.
/// 精确当前清单条目名称。
pub(super) const MANIFEST_ENTRY: &str = "manifest.json";
/// Exact current content entry name.
/// 精确当前内容条目名称。
pub(super) const CONTENT_ENTRY: &str = "content.jsonl";
/// The only permitted physical ZIP entries.
/// 唯一允许的物理 ZIP 条目。
const PACKAGE_ENTRIES: &[&str] = &[MANIFEST_ENTRY, CONTENT_ENTRY];
/// Bounded streaming checksum buffer, independent of accepted package size.
/// 有界流式校验缓冲，独立于允许套件大小。
const CHECKSUM_BUFFER_BYTES: usize = 128 * 1024;

/// Validated immutable in-memory package; no extracted paths or old-format artifacts exist.
/// 已验证不可变内存套件；不存在解压路径或旧格式产物。
pub(super) struct PreparedKnowledgePackage {
    /// Frozen public inspection identity.
    /// 冻结公共检查身份。
    pub(super) inspection: KnowledgePackageInspection,
    /// Fully validated unpublished content.
    /// 全部验证的未发布内容。
    pub(super) content: ValidatedPackageContent,
}

/// Reads only family/version before strict current-shape decoding.
/// 严格当前形状解码前仅读取家族和版本。
#[derive(Deserialize)]
struct PackageFormatHeader {
    /// Stable package family.
    /// 稳定套件家族。
    format: String,
    /// Declared package version.
    /// 声明套件版本。
    version: u32,
}

/// Requires an ordinary absolute directory; returns its canonical path or a trust error.
/// 要求普通绝对目录；返回规范路径或信任错误。
pub(super) fn canonical_existing_directory(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("ERR_KNOWLEDGE_PACKAGE_PATH_ABSOLUTE_REQUIRED".into());
    }
    // Metadata is read without following a final symbolic link or Windows reparse point.
    // 读取元数据时不跟随末级符号链接或 Windows 重解析点。
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || storage::metadata_is_link_or_reparse(&metadata) {
        return Err("ERR_KNOWLEDGE_PACKAGE_DIRECTORY_NOT_REGULAR".into());
    }
    fs::canonicalize(path).map_err(|error| error.to_string())
}

/// Requires an ordinary absolute .vkb file within the archive byte limit; returns its canonical identity.
/// 要求在归档字节上限内的普通绝对 .vkb 文件；返回规范身份。
pub(super) fn canonical_knowledge_package_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("ERR_KNOWLEDGE_PACKAGE_PATH_ABSOLUTE_REQUIRED".into());
    }
    // A user-selected archive is a single ordinary file, never a directory or link.
    // 用户选择的归档是单个普通文件，绝非目录或链接。
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || storage::metadata_is_link_or_reparse(&metadata) {
        return Err("ERR_KNOWLEDGE_PACKAGE_FILE_NOT_REGULAR".into());
    }
    if metadata.len() > MAX_KNOWLEDGE_PACKAGE_BYTES {
        return Err("ERR_KNOWLEDGE_PACKAGE_TOO_LARGE".into());
    }
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("vkb"))
    {
        return Err("ERR_KNOWLEDGE_PACKAGE_EXTENSION".into());
    }
    fs::canonicalize(path).map_err(|error| error.to_string())
}

/// Hashes an already-open file within the package bound and rewinds it; returns exact digest and size.
/// 在套件边界内对已打开文件求哈希并回卷；返回精确摘要和大小。
fn fingerprint(input: &mut File) -> Result<(String, u64), String> {
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    // Streaming avoids allocating the compressed archive merely to establish its identity.
    // 流式读取避免仅为建立身份而分配完整压缩归档。
    let mut digest = Sha256::new();
    // Buffer allocation has one fixed implementation bound.
    // 缓冲分配具有唯一固定实现边界。
    let mut buffer = [0u8; CHECKSUM_BUFFER_BYTES];
    // Count bytes actually read instead of trusting mutable file metadata.
    // 计数实际读取字节，而非信任可变文件元数据。
    let mut total = 0u64;
    loop {
        // Each read advances the same file handle later consumed by ZIP decoding.
        // 每次读取推进随后由 ZIP 解码消费的同一文件句柄。
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or("ERR_KNOWLEDGE_PACKAGE_TOO_LARGE")?;
        if total > MAX_KNOWLEDGE_PACKAGE_BYTES {
            return Err("ERR_KNOWLEDGE_PACKAGE_TOO_LARGE".into());
        }
        digest.update(&buffer[..count]);
    }
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    Ok((hex::encode(digest.finalize()), total))
}

/// Reads at most limit+1 decompressed bytes and verifies declared size; returns an explicit bounds error.
/// 最多读取上限加一的解压字节并验证声明大小；返回明确边界错误。
fn read_bounded_entry<R: Read>(
    input: &mut R,
    expected: u64,
    limit: u64,
) -> Result<Vec<u8>, String> {
    if expected > limit {
        return Err("ERR_KNOWLEDGE_PACKAGE_TOO_LARGE".into());
    }
    // ZIP entry size is verified against actual bytes after the bounded decoder reaches EOF.
    // 有界解码器达到结尾后，对照实际字节验证 ZIP 条目大小。
    let mut bytes = Vec::new();
    input
        .take(
            limit
                .checked_add(1)
                .ok_or("ERR_KNOWLEDGE_PACKAGE_TOO_LARGE")?,
        )
        .read_to_end(&mut bytes)
        .map_err(|error| format!("ERR_KNOWLEDGE_PACKAGE_DECODE: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err("ERR_KNOWLEDGE_PACKAGE_TOO_LARGE".into());
    }
    if bytes.len() as u64 != expected {
        return Err("ERR_KNOWLEDGE_PACKAGE_SIZE_MISMATCH".into());
    }
    Ok(bytes)
}

/// Validates one immutable archive and optional preview digest; returns current content only.
/// 验证一个不可变归档与可选预检摘要；仅返回当前内容。
pub(super) fn prepare_knowledge_package(
    path: &Path,
    expected_sha256: Option<&str>,
) -> Result<PreparedKnowledgePackage, String> {
    // Canonical identity is frozen before opening the single source file.
    // 打开唯一来源文件前冻结规范身份。
    let path = canonical_knowledge_package_path(path)?;
    // This handle is reused for both checksums and all ZIP entries.
    // 该句柄复用于两次校验和全部 ZIP 条目。
    let mut input = File::open(&path).map_err(|error| error.to_string())?;
    // The initial digest is the identity explicitly approved during inspection.
    // 初始摘要是检查期间明确批准的身份。
    let (sha256, byte_size) = fingerprint(&mut input)?;
    if let Some(expected) = expected_sha256 {
        vector::validate_sha256(expected)?;
        if expected != sha256 {
            return Err("ERR_KNOWLEDGE_PACKAGE_CHANGED".into());
        }
    }
    // The new format has exactly two named entries; all old artifact layouts are refused.
    // 新格式恰有两条命名条目；全部旧产物布局均被拒绝。
    let mut archive =
        ZipArchive::new(input).map_err(|error| format!("ERR_KNOWLEDGE_PACKAGE_ZIP: {error}"))?;
    if archive.len() != PACKAGE_ENTRIES.len() {
        return Err("ERR_KNOWLEDGE_PACKAGE_ENTRIES_UNSUPPORTED".into());
    }
    // Validate the closed entry set before decoding any user-controlled JSON.
    // 解码任何用户控制的 JSON 前验证封闭条目集合。
    let mut seen = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let Ok(name) = entry.name() else {
            return Err("ERR_KNOWLEDGE_PACKAGE_ENTRY_INVALID".into());
        };
        if entry.is_dir()
            || entry.enclosed_name().is_none()
            || !PACKAGE_ENTRIES.contains(&name.as_ref())
            || !seen.insert(name.into_owned())
            || entry
                .unix_mode()
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
        {
            return Err("ERR_KNOWLEDGE_PACKAGE_ENTRY_INVALID".into());
        }
    }
    // The manifest has its own smaller JSON bound.
    // 清单拥有更小的独立 JSON 边界。
    let manifest_bytes = {
        let mut entry = archive
            .by_name(MANIFEST_ENTRY)
            .map_err(|error| error.to_string())?;
        let expected = entry.size();
        read_bounded_entry(&mut entry, expected, MAX_KNOWLEDGE_PACKAGE_MANIFEST_BYTES)?
    };
    // Family and version are checked before the stricter current manifest fields.
    // 在更严格的当前清单字段前检查家族和版本。
    let header: PackageFormatHeader = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("ERR_KNOWLEDGE_PACKAGE_MANIFEST: {error}"))?;
    if header.format != KNOWLEDGE_PACKAGE_FORMAT || header.version != KNOWLEDGE_PACKAGE_VERSION {
        return Err(format!(
            "ERR_KNOWLEDGE_PACKAGE_VERSION_UNSUPPORTED: supported version is {KNOWLEDGE_PACKAGE_VERSION}; received {}",
            header.version
        ));
    }
    // Unknown fields and obsolete settings are rejected by the current strong types.
    // 未知字段和过时设置由当前强类型拒绝。
    let manifest: KnowledgePackageManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("ERR_KNOWLEDGE_PACKAGE_MANIFEST: {error}"))?;
    manifest.settings.validate()?;
    if !manifest.settings.index_mode.has_vectors() && manifest.settings.embedding.is_some() {
        return Err(
            "ERR_KNOWLEDGE_EMBEDDING_REQUIRED: nonvector packages omit the embedding route".into(),
        );
    }
    fts::validate_custom_words(&manifest.custom_words)?;
    if library::validate_library_name(&manifest.name)? != manifest.name {
        return Err("ERR_KNOWLEDGE_PACKAGE_NAME_NOT_CANONICAL".into());
    }
    if manifest
        .document_count
        .checked_add(manifest.folder_count)
        .ok_or("ERR_KNOWLEDGE_PACKAGE_ENTRY_LIMIT")?
        > MAX_KNOWLEDGE_PACKAGE_ENTRIES
    {
        return Err("ERR_KNOWLEDGE_PACKAGE_ENTRY_LIMIT".into());
    }
    // Both expanded entries share the overall package budget.
    // 两条展开条目共享整体套件预算。
    let content_bytes = {
        let mut entry = archive
            .by_name(CONTENT_ENTRY)
            .map_err(|error| error.to_string())?;
        let expected = entry.size();
        read_bounded_entry(
            &mut entry,
            expected,
            MAX_KNOWLEDGE_PACKAGE_BYTES - manifest_bytes.len() as u64,
        )?
    };
    if super::super::sha256_hex(&content_bytes) != manifest.content_sha256 {
        return Err("ERR_KNOWLEDGE_PACKAGE_CONTENT_DIGEST".into());
    }
    // JSONL parsing is bounded by both bytes and logical entity count.
    // JSONL 解析同时受字节和逻辑实体数限制。
    let text =
        std::str::from_utf8(&content_bytes).map_err(|_| "ERR_KNOWLEDGE_PACKAGE_CONTENT_UTF8")?;
    // Empty content represents a valid empty library and still imports as one change.
    // 空内容表示有效空库，仍作为一条变更导入。
    let mut records = Vec::new();
    for line in text.split_terminator('\n') {
        if records.len() >= MAX_KNOWLEDGE_PACKAGE_ENTRIES {
            return Err("ERR_KNOWLEDGE_PACKAGE_ENTRY_LIMIT".into());
        }
        if line.trim().is_empty() {
            return Err("ERR_KNOWLEDGE_PACKAGE_JSONL_EMPTY_RECORD".into());
        }
        records.push(
            serde_json::from_str::<KnowledgePackageRecord>(line)
                .map_err(|error| format!("ERR_KNOWLEDGE_PACKAGE_RECORD: {error}"))?,
        );
    }
    // Typed content preparation validates every path, summary and namespace before publication.
    // 类型化内容准备在发布前验证全部路径、摘要和命名空间。
    let content = content::prepare_package_content(records)?;
    if content.document_count() != manifest.document_count
        || content.folder_count() != manifest.folder_count
    {
        return Err("ERR_KNOWLEDGE_PACKAGE_COUNT_MISMATCH".into());
    }
    // A second checksum closes mutation during inspection, including in-place file changes.
    // 第二次校验封闭检查期间的变更，包括原地文件修改。
    let mut input = archive.into_inner();
    if fingerprint(&mut input)? != (sha256.clone(), byte_size) {
        return Err("ERR_KNOWLEDGE_PACKAGE_CHANGED".into());
    }
    Ok(PreparedKnowledgePackage {
        inspection: KnowledgePackageInspection {
            archive_path: path.display().to_string(),
            sha256,
            byte_size,
            manifest,
        },
        content,
    })
}

/// Writes a validated current snapshot to a unique .vkb destination; never overwrites an existing user file.
/// 将已验证当前快照写入唯一 .vkb 目标；绝不覆盖已有用户文件。
pub(super) fn write_package(
    library: KnowledgeLibrarySummary,
    snapshot: PackageExportContent,
    destination: &Path,
) -> Result<KnowledgePackageExportResult, String> {
    // Export destination is explicitly selected and must already be an ordinary absolute directory.
    // 导出目标为显式选择，必须已是普通绝对目录。
    let destination = canonical_existing_directory(destination)?;
    // Current active library settings are mandatory, never defaulted for invalid formats.
    // 当前活动知识库设置必填，非法格式绝不取默认值。
    let mut settings = library.settings.ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
    if !settings.index_mode.has_vectors() {
        settings.embedding = None;
    }
    // The manifest contains only current configuration and content integrity metadata.
    // 清单只包含当前配置和内容完整性元数据。
    let manifest = KnowledgePackageManifest {
        format: KNOWLEDGE_PACKAGE_FORMAT.into(),
        version: KNOWLEDGE_PACKAGE_VERSION,
        name: library.name,
        description: library.description,
        settings,
        custom_words: snapshot.custom_words,
        document_count: snapshot.document_count,
        folder_count: snapshot.folder_count,
        content_sha256: super::super::sha256_hex(&snapshot.jsonl),
        exported_at_ms: unix_epoch_millis()?,
    };
    // Manifest serialization is checked independently from the complete expanded payload.
    // 清单序列化独立于完整展开载荷进行检查。
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    if manifest_bytes.len() as u64 > MAX_KNOWLEDGE_PACKAGE_MANIFEST_BYTES
        || (manifest_bytes.len() as u64)
            .checked_add(snapshot.jsonl.len() as u64)
            .ok_or("ERR_KNOWLEDGE_PACKAGE_TOO_LARGE")?
            > MAX_KNOWLEDGE_PACKAGE_BYTES
    {
        return Err("ERR_KNOWLEDGE_PACKAGE_TOO_LARGE".into());
    }
    // A private same-directory temporary file permits atomic non-overwriting publication.
    // 同目录私有临时文件支持原子且不覆盖的发布。
    let mut temporary = tempfile::Builder::new()
        .prefix("knowledge-export-")
        .suffix(".tmp")
        .tempfile_in(&destination)
        .map_err(|error| error.to_string())?;
    {
        // ZIP writer owns no source database or live journal files.
        // ZIP 写入器不持有来源数据库或活动日志文件。
        let mut archive = ZipWriter::new(temporary.as_file_mut());
        // Both entries use the same declared portable compression.
        // 两条记录使用相同声明的可移植压缩。
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        archive
            .start_file(MANIFEST_ENTRY, options)
            .map_err(|error| error.to_string())?;
        archive
            .write_all(&manifest_bytes)
            .map_err(|error| error.to_string())?;
        archive
            .start_file(CONTENT_ENTRY, options)
            .map_err(|error| error.to_string())?;
        archive
            .write_all(&snapshot.jsonl)
            .map_err(|error| error.to_string())?;
        archive
            .finish()
            .map_err(|error| error.to_string())?
            .sync_all()
            .map_err(|error| error.to_string())?;
    }
    // Fingerprint the exact completed file before its immutable publication.
    // 不可变发布前对精确完成文件求指纹。
    let (sha256, byte_size) = fingerprint(temporary.as_file_mut())?;
    // Host-generated names avoid reserved names and path injection from human library labels.
    // 宿主生成名称避免人工库标签带来的保留名和路径注入。
    let archive_path = destination.join(format!("knowledge-{}.vkb", Uuid::new_v4()));
    temporary
        .persist_noclobber(&archive_path)
        .map_err(|error| error.to_string())?;
    Ok(KnowledgePackageInspection {
        archive_path: archive_path.display().to_string(),
        sha256,
        byte_size,
        manifest,
    })
}

#[cfg(test)]
mod tests;
