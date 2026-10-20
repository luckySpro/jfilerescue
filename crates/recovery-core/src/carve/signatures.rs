//! Signature library: built-in defaults + user overrides, stored as TOML.
//! Built-ins can be disabled but not deleted; "restore defaults" re-enables
//! them and keeps custom entries.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Header pattern with optional wildcard bytes (`??`) and a start offset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeaderPattern {
    /// Hex string, bytes separated by spaces, `??` = any byte. e.g. "FF D8 FF ??".
    pub hex: String,
    #[serde(default)]
    pub offset: usize,
}

impl HeaderPattern {
    pub fn bytes(&self) -> Vec<Option<u8>> {
        self.hex
            .split_whitespace()
            .filter_map(|t| {
                if t == "??" {
                    Some(None)
                } else {
                    u8::from_str_radix(t, 16).ok().map(Some)
                }
            })
            .collect()
    }
    /// Longest literal run starting at the first non-wildcard byte, and how many wildcard bytes precede it.
    pub fn literal_prefix(&self) -> (Vec<u8>, usize) {
        let b = self.bytes();
        let lead = b.iter().take_while(|x| x.is_none()).count();
        let lit: Vec<u8> = b[lead..]
            .iter()
            .take_while(|x| x.is_some())
            .map(|x| x.unwrap())
            .collect();
        (lit, lead)
    }
    pub fn matches(&self, data: &[u8]) -> bool {
        let b = self.bytes();
        if data.len() < self.offset + b.len() {
            return false;
        }
        b.iter()
            .enumerate()
            .all(|(i, x)| x.map(|v| v == data[self.offset + i]).unwrap_or(true))
    }
    pub fn validate(&self) -> Result<()> {
        let toks: Vec<&str> = self.hex.split_whitespace().collect();
        if toks.is_empty() {
            return Err(Error::InvalidInput("文件头不能为空".into()));
        }
        for t in &toks {
            if *t != "??" && u8::from_str_radix(t, 16).is_err() {
                return Err(Error::InvalidInput(format!("无效的十六进制字节: {}", t)));
            }
        }
        if self.literal_prefix().0.len() < 2 {
            return Err(Error::InvalidInput("文件头至少需要 2 个确定字节".into()));
        }
        Ok(())
    }
}

/// How to find the end of a file once its header is located.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LengthRule {
    /// Search forward for a footer; file ends after it.
    Footer { hex: String },
    /// Built-in structural parser (see `resolve.rs`).
    Parser { name: String },
    /// Fixed size.
    Fixed { size: u64 },
    /// Take `max_size` bytes (unverified).
    MaxSize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Signature {
    pub id: String,
    pub name: String,
    pub extensions: Vec<String>,
    pub headers: Vec<HeaderPattern>,
    pub length: LengthRule,
    #[serde(default = "default_min")]
    pub min_size: u64,
    #[serde(default = "default_max")]
    pub max_size: u64,
    /// Only accept headers at cluster/sector boundaries (true for most formats).
    #[serde(default = "default_true")]
    pub aligned: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub category: String,
}

fn default_min() -> u64 {
    64
}
fn default_max() -> u64 {
    512 << 20
}
fn default_true() -> bool {
    true
}

impl Signature {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(Error::InvalidInput("id 不能为空".into()));
        }
        if self.extensions.is_empty()
            || self
                .extensions
                .iter()
                .any(|e| e.trim().is_empty() || e.contains('.'))
        {
            return Err(Error::InvalidInput("扩展名不能为空且不含点".into()));
        }
        if self.headers.is_empty() {
            return Err(Error::InvalidInput("至少需要一个文件头".into()));
        }
        for h in &self.headers {
            h.validate()?;
        }
        if let LengthRule::Footer { hex } = &self.length {
            HeaderPattern {
                hex: hex.clone(),
                offset: 0,
            }
            .validate()?;
        }
        if self.max_size == 0 || self.min_size > self.max_size {
            return Err(Error::InvalidInput("大小范围无效".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignatureLibrary {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub signatures: Vec<Signature>,
}

/// User-level overrides persisted on disk: disabled built-ins + custom entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserOverrides {
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub custom: Vec<Signature>,
}

impl SignatureLibrary {
    pub fn builtin() -> Self {
        let mut lib: SignatureLibrary =
            toml::from_str(BUILTIN_TOML).expect("builtin signature TOML must parse");
        for s in &mut lib.signatures {
            s.builtin = true;
            s.enabled = true;
        }
        lib
    }

    /// Built-ins merged with the user overrides file (missing file = defaults).
    pub fn load(overrides_path: &Path) -> Result<Self> {
        let mut lib = Self::builtin();
        if overrides_path.exists() {
            let txt = std::fs::read_to_string(overrides_path)?;
            let ov: UserOverrides = toml::from_str(&txt)
                .map_err(|e| Error::InvalidInput(format!("签名配置文件解析失败: {}", e)))?;
            lib.apply(&ov);
        }
        Ok(lib)
    }

    pub fn apply(&mut self, ov: &UserOverrides) {
        for s in &mut self.signatures {
            if ov.disabled.contains(&s.id) {
                s.enabled = false;
            }
        }
        for c in &ov.custom {
            let mut c = c.clone();
            c.builtin = false;
            if let Some(existing) = self.signatures.iter_mut().find(|s| s.id == c.id) {
                *existing = c;
            } else {
                self.signatures.push(c);
            }
        }
    }

    pub fn overrides(&self) -> UserOverrides {
        UserOverrides {
            disabled: self
                .signatures
                .iter()
                .filter(|s| s.builtin && !s.enabled)
                .map(|s| s.id.clone())
                .collect(),
            custom: self
                .signatures
                .iter()
                .filter(|s| !s.builtin)
                .cloned()
                .collect(),
        }
    }

    /// Atomic save (temp file + rename) with one backup.
    pub fn save(&self, overrides_path: &Path) -> Result<()> {
        let txt =
            toml::to_string_pretty(&self.overrides()).map_err(|e| Error::Other(e.to_string()))?;
        if let Some(parent) = overrides_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = overrides_path.with_extension("toml.tmp");
        std::fs::write(&tmp, txt)?;
        if overrides_path.exists() {
            let bak = overrides_path.with_extension("toml.bak");
            let _ = std::fs::copy(overrides_path, bak);
        }
        std::fs::rename(&tmp, overrides_path)?;
        Ok(())
    }

    pub fn enabled(&self) -> Vec<&Signature> {
        self.signatures.iter().filter(|s| s.enabled).collect()
    }

    pub fn get(&self, id: &str) -> Option<&Signature> {
        self.signatures.iter().find(|s| s.id == id)
    }

    pub fn upsert(&mut self, mut sig: Signature) -> Result<()> {
        sig.validate()?;
        if let Some(e) = self.signatures.iter_mut().find(|s| s.id == sig.id) {
            if e.builtin {
                // Editing a built-in: keep builtin flag, replace rules.
                sig.builtin = true;
            }
            *e = sig;
        } else {
            sig.builtin = false;
            self.signatures.push(sig);
        }
        Ok(())
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) -> Result<()> {
        let s = self
            .signatures
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::InvalidInput(format!("签名不存在: {}", id)))?;
        s.enabled = on;
        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        let idx = self
            .signatures
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| Error::InvalidInput(format!("签名不存在: {}", id)))?;
        if self.signatures[idx].builtin {
            self.signatures[idx].enabled = false;
        } else {
            self.signatures.remove(idx);
        }
        Ok(())
    }

    pub fn restore_defaults(&mut self) {
        let custom: Vec<Signature> = self
            .signatures
            .iter()
            .filter(|s| !s.builtin)
            .cloned()
            .collect();
        *self = Self::builtin();
        self.signatures.extend(custom);
    }

    /// Export the full effective library (for sharing between machines).
    pub fn export_toml(&self) -> String {
        toml::to_string_pretty(&self.overrides()).unwrap_or_default()
    }

    pub fn import_toml(&mut self, txt: &str) -> Result<usize> {
        let ov: UserOverrides = toml::from_str(txt)
            .map_err(|e| Error::InvalidInput(format!("导入内容解析失败: {}", e)))?;
        for c in &ov.custom {
            c.validate()?;
        }
        let n = ov.custom.len();
        self.apply(&ov);
        Ok(n)
    }
}

/// Built-in signature table. Patterns are public format facts; the carving
/// implementation is our own. Parser names map to `resolve.rs`.
pub const BUILTIN_TOML: &str = r##"
version = 1

# ---------- Images ----------
[[signatures]]
id = "jpg"
name = "JPEG 图片"
category = "image"
extensions = ["jpg", "jpeg"]
headers = [{ hex = "FF D8 FF" }]
length = { kind = "parser", name = "jpeg" }
min_size = 1024
max_size = 268435456

[[signatures]]
id = "png"
name = "PNG 图片"
category = "image"
extensions = ["png"]
headers = [{ hex = "89 50 4E 47 0D 0A 1A 0A" }]
length = { kind = "parser", name = "png" }
max_size = 268435456

[[signatures]]
id = "gif"
name = "GIF 图片"
category = "image"
extensions = ["gif"]
headers = [{ hex = "47 49 46 38 37 61" }, { hex = "47 49 46 38 39 61" }]
length = { kind = "footer", hex = "00 3B" }
max_size = 67108864

[[signatures]]
id = "bmp"
name = "BMP 图片"
category = "image"
extensions = ["bmp"]
headers = [{ hex = "42 4D ?? ?? ?? ?? 00 00 00 00" }]
length = { kind = "parser", name = "bmp" }
max_size = 268435456

[[signatures]]
id = "tiff"
name = "TIFF 图片"
category = "image"
extensions = ["tif", "tiff"]
headers = [{ hex = "49 49 2A 00" }, { hex = "4D 4D 00 2A" }]
length = { kind = "max_size" }
max_size = 536870912

[[signatures]]
id = "webp"
name = "WebP 图片"
category = "image"
extensions = ["webp"]
headers = [{ hex = "52 49 46 46 ?? ?? ?? ?? 57 45 42 50" }]
length = { kind = "parser", name = "riff" }
max_size = 134217728

[[signatures]]
id = "heic"
name = "HEIC/HEIF 图片"
category = "image"
extensions = ["heic", "heif"]
headers = [{ hex = "?? ?? ?? ?? 66 74 79 70 68 65 69 63" }, { hex = "?? ?? ?? ?? 66 74 79 70 6D 69 66 31" }]
length = { kind = "parser", name = "bmff" }
max_size = 134217728

[[signatures]]
id = "psd"
name = "Photoshop 文档"
category = "image"
extensions = ["psd"]
headers = [{ hex = "38 42 50 53 00 01" }]
length = { kind = "max_size" }
max_size = 2147483648

[[signatures]]
id = "cr2"
name = "Canon RAW"
category = "image"
extensions = ["cr2"]
headers = [{ hex = "49 49 2A 00 10 00 00 00 43 52" }]
length = { kind = "max_size" }
max_size = 134217728

[[signatures]]
id = "nef"
name = "Nikon RAW"
category = "image"
extensions = ["nef"]
headers = [{ hex = "4D 4D 00 2A 00 00 00 08 00" }]
length = { kind = "max_size" }
max_size = 134217728

[[signatures]]
id = "dng"
name = "Adobe DNG"
category = "image"
extensions = ["dng"]
headers = [{ hex = "49 49 2A 00 08 00 00 00" }]
length = { kind = "max_size" }
max_size = 268435456

[[signatures]]
id = "svg"
name = "SVG 矢量图"
category = "image"
extensions = ["svg"]
headers = [{ hex = "3C 73 76 67 20" }, { hex = "3C 3F 78 6D 6C ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? 3C 73 76 67" }]
length = { kind = "footer", hex = "3C 2F 73 76 67 3E" }
max_size = 33554432

# ---------- Documents ----------
[[signatures]]
id = "pdf"
name = "PDF 文档"
category = "document"
extensions = ["pdf"]
headers = [{ hex = "25 50 44 46 2D" }]
length = { kind = "parser", name = "pdf" }
max_size = 1073741824

[[signatures]]
id = "ooxml"
name = "Office Open XML (docx/xlsx/pptx)"
category = "document"
extensions = ["docx", "xlsx", "pptx"]
headers = [{ hex = "50 4B 03 04 14 00 06 00" }, { hex = "50 4B 03 04 14 00 00 00 08 00" }, { hex = "50 4B 03 04 14 00 08 08 08 00" }]
length = { kind = "parser", name = "zip" }
min_size = 2048
max_size = 1073741824

[[signatures]]
id = "ole"
name = "Office 97-2003 (doc/xls/ppt)"
category = "document"
extensions = ["doc", "xls", "ppt"]
headers = [{ hex = "D0 CF 11 E0 A1 B1 1A E1" }]
length = { kind = "parser", name = "ole" }
max_size = 536870912

[[signatures]]
id = "rtf"
name = "RTF 文档"
category = "document"
extensions = ["rtf"]
headers = [{ hex = "7B 5C 72 74 66 31" }]
length = { kind = "footer", hex = "7D 0D 0A" }
max_size = 134217728

[[signatures]]
id = "odf"
name = "OpenDocument (odt/ods/odp)"
category = "document"
extensions = ["odt", "ods", "odp"]
headers = [{ hex = "50 4B 03 04 14 00 00 08 00 00 ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? 08 00 00 00 6D 69 6D 65 74 79 70 65" }]
length = { kind = "parser", name = "zip" }
max_size = 1073741824

[[signatures]]
id = "epub"
name = "EPUB 电子书"
category = "document"
extensions = ["epub"]
headers = [{ hex = "50 4B 03 04 ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? 6D 69 6D 65 74 79 70 65 61 70 70 6C 69 63 61 74 69 6F 6E 2F 65 70 75 62" }]
length = { kind = "parser", name = "zip" }
max_size = 536870912

[[signatures]]
id = "xml"
name = "XML 文本"
category = "document"
extensions = ["xml"]
headers = [{ hex = "3C 3F 78 6D 6C 20 76 65 72 73 69 6F 6E" }]
length = { kind = "parser", name = "text" }
max_size = 67108864
enabled = false

[[signatures]]
id = "html"
name = "HTML 文本"
category = "document"
extensions = ["html"]
headers = [{ hex = "3C 21 44 4F 43 54 59 50 45 20 68 74 6D 6C" }, { hex = "3C 21 64 6F 63 74 79 70 65 20 68 74 6D 6C" }, { hex = "3C 68 74 6D 6C" }]
length = { kind = "footer", hex = "3C 2F 68 74 6D 6C 3E" }
max_size = 33554432
enabled = false

# ---------- Archives ----------
[[signatures]]
id = "zip"
name = "ZIP 压缩包"
category = "archive"
extensions = ["zip"]
headers = [{ hex = "50 4B 03 04" }]
length = { kind = "parser", name = "zip" }
max_size = 4294967296

[[signatures]]
id = "rar"
name = "RAR 压缩包"
category = "archive"
extensions = ["rar"]
headers = [{ hex = "52 61 72 21 1A 07 00" }, { hex = "52 61 72 21 1A 07 01 00" }]
length = { kind = "max_size" }
max_size = 4294967296

[[signatures]]
id = "7z"
name = "7-Zip 压缩包"
category = "archive"
extensions = ["7z"]
headers = [{ hex = "37 7A BC AF 27 1C" }]
length = { kind = "parser", name = "sevenzip" }
max_size = 4294967296

[[signatures]]
id = "gz"
name = "GZIP 压缩包"
category = "archive"
extensions = ["gz"]
headers = [{ hex = "1F 8B 08" }]
length = { kind = "max_size" }
max_size = 2147483648

[[signatures]]
id = "tar"
name = "TAR 归档"
category = "archive"
extensions = ["tar"]
headers = [{ hex = "75 73 74 61 72 00 30 30", offset = 257 }, { hex = "75 73 74 61 72 20 20 00", offset = 257 }]
length = { kind = "parser", name = "tar" }
max_size = 4294967296

[[signatures]]
id = "xz"
name = "XZ 压缩包"
category = "archive"
extensions = ["xz"]
headers = [{ hex = "FD 37 7A 58 5A 00" }]
length = { kind = "max_size" }
max_size = 2147483648

[[signatures]]
id = "bz2"
name = "BZip2 压缩包"
category = "archive"
extensions = ["bz2"]
headers = [{ hex = "42 5A 68 39 31 41 59 26 53 59" }]
length = { kind = "max_size" }
max_size = 2147483648

[[signatures]]
id = "dmg"
name = "Apple 磁盘映像"
category = "archive"
extensions = ["dmg"]
headers = [{ hex = "78 01 73 0D 62 62 60" }]
length = { kind = "max_size" }
max_size = 8589934592
enabled = false

# ---------- Audio / Video ----------
[[signatures]]
id = "mp3"
name = "MP3 音频"
category = "audio"
extensions = ["mp3"]
headers = [{ hex = "49 44 33" }]
length = { kind = "max_size" }
max_size = 268435456

[[signatures]]
id = "flac"
name = "FLAC 音频"
category = "audio"
extensions = ["flac"]
headers = [{ hex = "66 4C 61 43 00 00 00 22" }]
length = { kind = "max_size" }
max_size = 1073741824

[[signatures]]
id = "wav"
name = "WAV 音频"
category = "audio"
extensions = ["wav"]
headers = [{ hex = "52 49 46 46 ?? ?? ?? ?? 57 41 56 45" }]
length = { kind = "parser", name = "riff" }
max_size = 4294967296

[[signatures]]
id = "ogg"
name = "OGG 音频"
category = "audio"
extensions = ["ogg"]
headers = [{ hex = "4F 67 67 53 00 02" }]
length = { kind = "max_size" }
max_size = 536870912

[[signatures]]
id = "m4a"
name = "M4A/AAC 音频"
category = "audio"
extensions = ["m4a"]
headers = [{ hex = "?? ?? ?? ?? 66 74 79 70 4D 34 41 20" }]
length = { kind = "parser", name = "bmff" }
max_size = 1073741824

[[signatures]]
id = "mp4"
name = "MP4/MOV 视频"
category = "video"
extensions = ["mp4", "mov", "m4v"]
headers = [{ hex = "?? ?? ?? ?? 66 74 79 70 69 73 6F 6D" }, { hex = "?? ?? ?? ?? 66 74 79 70 6D 70 34" }, { hex = "?? ?? ?? ?? 66 74 79 70 71 74 20 20" }, { hex = "?? ?? ?? ?? 66 74 79 70 4D 53 4E 56" }, { hex = "?? ?? ?? ?? 66 74 79 70 61 76 63 31" }, { hex = "?? ?? ?? ?? 66 74 79 70 33 67 70" }]
length = { kind = "parser", name = "bmff" }
max_size = 17179869184

[[signatures]]
id = "avi"
name = "AVI 视频"
category = "video"
extensions = ["avi"]
headers = [{ hex = "52 49 46 46 ?? ?? ?? ?? 41 56 49 20" }]
length = { kind = "parser", name = "riff" }
max_size = 4294967296

[[signatures]]
id = "mkv"
name = "Matroska/WebM 视频"
category = "video"
extensions = ["mkv", "webm"]
headers = [{ hex = "1A 45 DF A3" }]
length = { kind = "max_size" }
max_size = 17179869184

[[signatures]]
id = "wmv"
name = "WMV/ASF 视频"
category = "video"
extensions = ["wmv", "asf"]
headers = [{ hex = "30 26 B2 75 8E 66 CF 11 A6 D9 00 AA 00 62 CE 6C" }]
length = { kind = "parser", name = "asf" }
max_size = 8589934592

[[signatures]]
id = "flv"
name = "Flash 视频"
category = "video"
extensions = ["flv"]
headers = [{ hex = "46 4C 56 01" }]
length = { kind = "max_size" }
max_size = 4294967296

# ---------- Executables / Misc ----------
[[signatures]]
id = "exe"
name = "Windows 可执行文件"
category = "binary"
extensions = ["exe", "dll"]
headers = [{ hex = "4D 5A 90 00 03 00 00 00 04 00 00 00 FF FF" }]
length = { kind = "parser", name = "pe" }
max_size = 536870912
enabled = false

[[signatures]]
id = "sqlite"
name = "SQLite 数据库"
category = "database"
extensions = ["sqlite", "db"]
headers = [{ hex = "53 51 4C 69 74 65 20 66 6F 72 6D 61 74 20 33 00" }]
length = { kind = "parser", name = "sqlite" }
max_size = 4294967296

[[signatures]]
id = "pst"
name = "Outlook 数据文件"
category = "database"
extensions = ["pst"]
headers = [{ hex = "21 42 44 4E" }]
length = { kind = "max_size" }
max_size = 17179869184
enabled = false

[[signatures]]
id = "ttf"
name = "TrueType 字体"
category = "binary"
extensions = ["ttf"]
headers = [{ hex = "00 01 00 00 00 ?? ?? ?? ?? 00" }]
length = { kind = "max_size" }
max_size = 67108864
enabled = false

[[signatures]]
id = "otf"
name = "OpenType 字体"
category = "binary"
extensions = ["otf"]
headers = [{ hex = "4F 54 54 4F 00" }]
length = { kind = "max_size" }
max_size = 67108864
enabled = false

[[signatures]]
id = "class"
name = "Java Class"
category = "binary"
extensions = ["class"]
headers = [{ hex = "CA FE BA BE 00 00 00" }]
length = { kind = "max_size" }
max_size = 16777216
enabled = false

[[signatures]]
id = "dwg"
name = "AutoCAD 图纸"
category = "document"
extensions = ["dwg"]
headers = [{ hex = "41 43 31 30" }]
length = { kind = "max_size" }
max_size = 536870912

[[signatures]]
id = "ai"
name = "Adobe Illustrator"
category = "image"
extensions = ["ai"]
headers = [{ hex = "25 50 44 46 2D 31 2E ?? 0A 25 C7 EC 8F A2" }]
length = { kind = "parser", name = "pdf" }
max_size = 1073741824
enabled = false

[[signatures]]
id = "indd"
name = "Adobe InDesign"
category = "document"
extensions = ["indd"]
headers = [{ hex = "06 06 ED F5 D8 1D 46 E5 BD 31 EF E7 FE 74 B7 1D" }]
length = { kind = "max_size" }
max_size = 2147483648
enabled = false

[[signatures]]
id = "vmdk"
name = "VMware 磁盘"
category = "binary"
extensions = ["vmdk"]
headers = [{ hex = "4B 44 4D 56" }]
length = { kind = "max_size" }
max_size = 68719476736
enabled = false

[[signatures]]
id = "iso"
name = "ISO 光盘映像"
category = "archive"
extensions = ["iso"]
headers = [{ hex = "43 44 30 30 31", offset = 32769 }]
length = { kind = "max_size" }
max_size = 8589934592
enabled = false

[[signatures]]
id = "eml"
name = "邮件 (EML)"
category = "document"
extensions = ["eml"]
headers = [{ hex = "52 65 74 75 72 6E 2D 50 61 74 68 3A 20" }, { hex = "52 65 63 65 69 76 65 64 3A 20 66 72 6F 6D" }]
length = { kind = "parser", name = "text" }
max_size = 67108864
enabled = false

[[signatures]]
id = "json"
name = "JSON 文本"
category = "document"
extensions = ["json"]
headers = [{ hex = "7B 0A 20 20 22" }, { hex = "7B 22" }]
length = { kind = "parser", name = "text" }
max_size = 33554432
enabled = false

[[signatures]]
id = "csv_utf8bom"
name = "CSV/文本 (UTF-8 BOM)"
category = "document"
extensions = ["csv", "txt"]
headers = [{ hex = "EF BB BF" }]
length = { kind = "parser", name = "text" }
max_size = 67108864
enabled = false

[[signatures]]
id = "pptx_media"
name = "Office 嵌入媒体 (EMF)"
category = "image"
extensions = ["emf"]
headers = [{ hex = "01 00 00 00 ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? ?? 20 45 4D 46" }]
length = { kind = "max_size" }
max_size = 67108864
enabled = false
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_parses_and_validates() {
        let lib = SignatureLibrary::builtin();
        assert!(lib.signatures.len() > 40);
        for s in &lib.signatures {
            s.validate().unwrap_or_else(|e| panic!("{}: {}", s.id, e));
            assert!(s.builtin);
        }
        let ids: std::collections::HashSet<_> = lib.signatures.iter().map(|s| &s.id).collect();
        assert_eq!(ids.len(), lib.signatures.len(), "duplicate ids");
    }

    #[test]
    fn header_wildcards() {
        let h = HeaderPattern {
            hex: "?? ?? ?? ?? 66 74 79 70".into(),
            offset: 0,
        };
        assert_eq!(h.literal_prefix(), (b"ftyp".to_vec(), 4));
        assert!(h.matches(b"\x00\x00\x00\x18ftypisom"));
        assert!(!h.matches(b"\x00\x00\x00\x18ftxpisom"));
    }

    #[test]
    fn overrides_roundtrip() {
        let mut lib = SignatureLibrary::builtin();
        lib.set_enabled("jpg", false).unwrap();
        lib.upsert(Signature {
            id: "custom1".into(),
            name: "Custom".into(),
            extensions: vec!["cst".into()],
            headers: vec![HeaderPattern {
                hex: "AA BB CC".into(),
                offset: 0,
            }],
            length: LengthRule::MaxSize,
            min_size: 64,
            max_size: 1024,
            aligned: true,
            enabled: true,
            builtin: false,
            category: "".into(),
        })
        .unwrap();
        let ov = lib.overrides();
        assert_eq!(ov.disabled, vec!["jpg".to_string()]);
        assert_eq!(ov.custom.len(), 1);
        let mut lib2 = SignatureLibrary::builtin();
        lib2.apply(&ov);
        assert!(!lib2.get("jpg").unwrap().enabled);
        assert!(lib2.get("custom1").is_some());
        lib2.delete("jpg").unwrap(); // builtin → disabled only
        assert!(lib2.get("jpg").is_some());
        lib2.delete("custom1").unwrap();
        assert!(lib2.get("custom1").is_none());
        lib2.restore_defaults();
        assert!(lib2.get("jpg").unwrap().enabled);
    }
}
