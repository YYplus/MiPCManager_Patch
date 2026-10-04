//! 内置安装来源与兼容要求；更新地址时一并更新对应 SHA-256。

use crate::i18n::{self, Lang};
use crate::infra::download::{self, DownloadControl};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendedInstaller {
    PcManager,
    PcContinuity,
    HyperConnectBeta,
    Xiaoai,
}

impl RecommendedInstaller {
    pub const MANAGER_VARIANTS: [Self; 3] =
        [Self::PcManager, Self::PcContinuity, Self::HyperConnectBeta];
    pub const ALL: [Self; 4] = [
        Self::PcManager,
        Self::PcContinuity,
        Self::HyperConnectBeta,
        Self::Xiaoai,
    ];

    pub fn url(self) -> &'static str {
        match self {
            Self::PcManager => {
                "https://cdn.cnbj1.fds.api.mi-img.com/ota-packages/TS4L_XiaomiPCManager_feature_p52_5.8.1.130_b5a04836.exe"
            }
            Self::PcContinuity => {
                "https://cdn.cnbj1.fds.api.mi-img.com/ota-packages/QKTo_PcContinuity_hotfix_88a510d20d_1.1.2.36_d887cad6.exe"
            }
            Self::HyperConnectBeta => {
                "https://hyper-connect.cnbj2m.mi-fds.com/hyper-connect/download/HyperConnect_AI.exe"
            }
            Self::Xiaoai => {
                "https://cdn.cnbj1.fds.api.mi-img.com/ota-packages/s6bK_XiaoaiAgent_3.5.0.220_31444585.exe"
            }
        }
    }

    pub fn checksum(self) -> &'static str {
        match self {
            Self::PcManager => "d812cf30805d172474eb33b2f97f37129593f9d50f09d4224f73d530a2cb40e9",
            Self::PcContinuity => {
                "0abde77d8bb9ac32e8d5a28d5cfa967ba6a59572ef392e42aae5825a133718e0"
            }
            Self::HyperConnectBeta => {
                "557a1a91709f4ab35ee6ad38f06073736633df26082071ae98000ed064f2169e"
            }
            Self::Xiaoai => "25f52d8576cc5d2c95b87a7ef49e36dd7f74342389c8acc53ab8a15048cd5d70",
        }
    }

    pub fn label(self, lang: Lang) -> &'static str {
        i18n::tr(
            match self {
                Self::PcManager => "install.source.manager",
                Self::PcContinuity => "install.source.continuity",
                Self::HyperConnectBeta => "install.source.hyperconnect-beta",
                Self::Xiaoai => "install.source.xiaoai",
            },
            lang,
        )
    }

    pub fn requirement(self, lang: Lang) -> &'static str {
        i18n::tr(
            match self {
                Self::PcContinuity => "install.requirement.continuity",
                Self::HyperConnectBeta => "install.requirement.hyperconnect-beta",
                Self::PcManager | Self::Xiaoai => "install.requirement.installer",
            },
            lang,
        )
    }
}

pub fn checksum_for_url(url: &str) -> Option<&'static str> {
    RecommendedInstaller::ALL
        .into_iter()
        .find(|source| source.url() == url)
        .map(RecommendedInstaller::checksum)
}

/// 下载缓存放在 Windows 临时目录，按来源及校验值隔离，便于系统清理。
pub fn download_dir(url: &str) -> Result<PathBuf> {
    let identity = format!(
        "{:x}",
        Sha256::digest(format!(
            "{url}\n{}",
            checksum_for_url(url).unwrap_or_default()
        ))
    );
    Ok(std::env::temp_dir()
        .join("MiPCManager_Patch")
        .join("downloads")
        .join(identity))
}

/// 启动前校验内置来源，并保持读锁直到安装器启动完成。
pub fn protect_downloaded_installer(
    installer: &Path,
    url: &str,
    control: &DownloadControl,
) -> Result<File> {
    let mut file = download::lock_file_for_read(installer)?;
    if let Some(expected) = checksum_for_url(url) {
        download::verify_open_file(&mut file, expected, control)?;
    }
    control.check_cancelled()?;
    Ok(file)
}
