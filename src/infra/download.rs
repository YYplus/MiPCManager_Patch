//! 单次 aria2 HTTP(S) 下载：最多八路、实际传输进度、取消与断点续传。

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const PAYLOAD: &[u8] = include_bytes!("../../assets/aria2/aria2c.exe.gz");
const PAYLOAD_SHA256: &str = "be2099c214f63a3cb4954b09a0becd6e2e34660b886d4c898d260febfe9d70c2";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const CHECKSUM_MISMATCH: &str = "SHA-256 不匹配；文件可能损坏或官方安装包已更新，不会运行该安装包";

#[derive(Clone, Default)]
pub struct DownloadControl(Arc<AtomicBool>);

impl DownloadControl {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.0.load(Ordering::Relaxed) {
            bail!("下载已取消；再次下载相同地址可续传");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DownloadPhase {
    #[default]
    Preparing,
    Downloading,
    Verifying,
    Complete,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DownloadProgress {
    pub phase: DownloadPhase,
    pub completed: u64,
    pub total: u64,
    pub bytes_per_second: u64,
    pub connections: u32,
}

impl DownloadProgress {
    pub fn fraction(self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.completed as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        }
    }
}

/// 不覆盖已有文件；有固定 SHA-256 时可复用通过校验的完整下载。
/// `.part` / `.aria2` / `.meta` 只属于这个 URL，失败或取消后保留用于续传。
pub fn download(
    url: &str,
    target: &Path,
    checksum: Option<&str>,
    control: &DownloadControl,
    mut progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf> {
    validate_url(url)?;
    control.check_cancelled()?;
    let parent = target.parent().context("无法确定下载目录")?;
    fs::create_dir_all(parent).context("无法创建下载目录")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sidecar(target, ".download.lock"))
        .context("无法打开下载锁")?;
    lock.try_lock().context("该安装包正在由另一个操作下载")?;
    progress(DownloadProgress::default());
    if target.try_exists()? {
        let expected = checksum.context("下载目标已存在；请选择本地安装，或先移走该文件")?;
        let total = fs::metadata(target)?.len();
        progress(DownloadProgress {
            phase: DownloadPhase::Verifying,
            completed: total,
            total,
            ..Default::default()
        });
        verify_checksum(target, expected, control)?;
        report_complete(target, &mut progress)?;
        return Ok(target.to_path_buf());
    }

    let part = sidecar(target, ".part");
    let metadata = sidecar(target, ".download.meta");
    let identity = format!(
        "{:x}",
        Sha256::digest(format!("{url}\n{}", checksum.unwrap_or_default()))
    );
    match fs::read_to_string(&metadata) {
        Ok(saved) if saved == identity => {}
        Ok(_) => bail!("存在来自不同地址的未完成下载；请先移走 {}", part.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if part.try_exists()? || sidecar(&part, ".aria2").try_exists()? {
                bail!("未完成下载缺少来源标记；请先移走 {}", part.display());
            }
            let mut marker = tempfile::NamedTempFile::new_in(parent)?;
            marker.write_all(identity.as_bytes())?;
            marker.as_file().sync_all()?;
            marker
                .persist_noclobber(&metadata)
                .context("无法保存下载来源标记")?;
        }
        Err(error) => return Err(error).context("无法读取下载来源标记"),
    }
    if download_partial(url, &part, checksum, control, &mut progress, 8)?
        == TransferOutcome::RangeUnsupported
    {
        control.check_cancelled()?;
        // 前一个子进程已退出。服务器不支持 Range 时，残留分块无法续传，重新单路下载。
        for path in [&part, &sidecar(&part, ".aria2")] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("无法清理服务器不支持的分块下载"),
            }
        }
        progress(DownloadProgress::default());
        // no-Range 回退不向 aria2 提供完整性元数据，避免它据此构造整文件 Range；
        // SHA-256 仍在传输完成后由本进程强制校验，校验通过前绝不会晋升为最终 .exe。
        download_partial(url, &part, None, control, &mut progress, 1)?;
    }
    if let Some(expected) = checksum {
        let total = fs::metadata(&part)?.len();
        progress(DownloadProgress {
            phase: DownloadPhase::Verifying,
            completed: total,
            total,
            ..Default::default()
        });
        verify_checksum(&part, expected, control)?;
    }
    control.check_cancelled()?;
    // persist_noclobber 防止下载期间其他进程创建的同名文件被覆盖。
    let temporary = tempfile::TempPath::try_from_path(&part)?;
    if let Err(error) = temporary.persist_noclobber(target) {
        let reason = error.error;
        error.path.keep().context("无法保留未完成下载")?;
        return Err(reason).context("无法保存安装包（不会覆盖同名文件）");
    }
    fs::remove_file(metadata).context("安装包已保存，但无法删除下载来源标记")?;
    report_complete(target, &mut progress)?;
    Ok(target.to_path_buf())
}

pub fn validate_url(url: &str) -> Result<()> {
    let lower = url.to_ascii_lowercase();
    if (!lower.starts_with("http://") && !lower.starts_with("https://"))
        || url.chars().any(char::is_control)
    {
        bail!("下载地址必须是有效的 HTTP(S) 地址");
    }
    Ok(())
}

pub fn verify_checksum(path: &Path, expected: &str, control: &DownloadControl) -> Result<()> {
    let mut file = lock_file_for_read(path)?;
    verify_open_file(&mut file, expected, control)
        .with_context(|| format!("校验失败：{}", path.display()))
}

/// 保持此句柄直到启动完成，避免已校验的可执行文件被并发替换。
pub fn lock_file_for_read(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1); // FILE_SHARE_READ
    }
    options
        .open(path)
        .with_context(|| format!("无法保护下载文件 {}", path.display()))
}

pub fn verify_open_file(file: &mut File, expected: &str, control: &DownloadControl) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut block = [0_u8; 64 * 1024];
    loop {
        control.check_cancelled()?;
        let count = file.read(&mut block)?;
        if count == 0 {
            break;
        }
        hash.update(&block[..count]);
    }
    if !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected) {
        bail!(CHECKSUM_MISMATCH);
    }
    Ok(())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

fn report_complete(path: &Path, progress: &mut impl FnMut(DownloadProgress)) -> Result<()> {
    let total = fs::metadata(path)?.len();
    progress(DownloadProgress {
        phase: DownloadPhase::Complete,
        completed: total,
        total,
        ..Default::default()
    });
    Ok(())
}

fn unpack_payload(path: &Path) -> Result<()> {
    let mut decoder = flate2::read::GzDecoder::new(PAYLOAD);
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    std::io::copy(&mut decoder, &mut file).context("无法解压内嵌下载器")?;
    file.sync_all()?;
    drop(file);
    verify_checksum(path, PAYLOAD_SHA256, &DownloadControl::default())
}

#[derive(PartialEq, Eq)]
enum TransferOutcome {
    Complete,
    RangeUnsupported,
}

fn download_partial(
    url: &str,
    part: &Path,
    checksum: Option<&str>,
    control: &DownloadControl,
    progress: &mut impl FnMut(DownloadProgress),
    connections: u32,
) -> Result<TransferOutcome> {
    let payload_dir = tempfile::Builder::new()
        .prefix("mipcm-aria2-")
        .rand_bytes(32)
        .tempdir()?;
    let executable = payload_dir.path().join("aria2c.exe");
    unpack_payload(&executable)?;
    #[cfg(windows)]
    let executable_lock = {
        use std::os::windows::fs::OpenOptionsExt;
        // 校验后禁止其他进程修改或替换即将执行的文件，并保持到子进程退出。
        let file = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&executable)?;
        verify_checksum(&executable, PAYLOAD_SHA256, control)?;
        file
    };
    let executable = executable_path(&executable)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random)
        .map_err(|error| anyhow::anyhow!("无法生成下载会话令牌：{error}"))?;
    let secret = format!("{:x}", Sha256::digest(random));
    drop(listener);
    let rpc = RpcClient { address, secret };
    let mut command = Command::new(&executable);
    command
        .current_dir(payload_dir.path())
        .args([
            "--no-conf=true",
            "--no-netrc=true",
            "--enable-rpc=true",
            "--rpc-listen-all=false",
            "--rpc-allow-origin-all=false",
            "--min-split-size=1M",
            "--max-concurrent-downloads=1",
            "--file-allocation=none",
            "--auto-file-renaming=false",
            "--allow-overwrite=false",
            "--always-resume=false",
            "--auto-save-interval=1",
            "--max-tries=5",
            "--retry-wait=2",
            "--connect-timeout=15",
            "--timeout=30",
            "--check-certificate=true",
            "--follow-torrent=false",
            "--follow-metalink=false",
            "--enable-dht=false",
            "--enable-dht6=false",
            "--enable-peer-exchange=false",
            "--show-console-readout=false",
            "--console-log-level=error",
        ])
        .arg(if connections == 1 {
            "--continue=false"
        } else {
            "--continue=true"
        })
        .arg(format!("--split={connections}"))
        .arg(format!("--max-connection-per-server={connections}"))
        .arg(format!("--rpc-listen-port={}", address.port()))
        .arg(format!("--rpc-secret={}", rpc.secret))
        .arg(format!("--stop-with-process={}", std::process::id()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(test)]
    command
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("ftp_proxy");
    let mut child = ChildGuard(command.spawn().context("无法启动内嵌 aria2 下载器")?);
    let start = Instant::now();
    loop {
        control.check_cancelled()?;
        if let Some(status) = child.0.try_wait()? {
            bail!("aria2 启动失败（退出码：{status}）");
        }
        match rpc.call("aria2.getVersion", vec![]) {
            Ok(_) => break,
            Err(error) if start.elapsed() >= Duration::from_secs(10) => {
                return Err(error).context("无法连接下载会话");
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
    let directory = std::path::absolute(part.parent().context("无法确定下载目录")?)?;
    let directory = directory
        .to_str()
        .context("下载目录包含无效的 Unicode 字符")?;
    let mut options = json!({"dir": directory, "out": part.file_name().and_then(|name| name.to_str()).context("无法确定下载文件名")?});
    if let Some(expected) = checksum {
        options["checksum"] = json!(format!("sha-256={expected}"));
        options["check-integrity"] = json!("true");
    }
    let gid = rpc
        .call("aria2.addUri", vec![json!([url]), options])?
        .as_str()
        .context("下载器未返回任务 ID")?
        .to_owned();
    let result = (|| {
        loop {
            if let Err(error) = control.check_cancelled() {
                // forcePause 保存分块控制文件；优雅退出后仍可续传。
                let _ = rpc.call("aria2.forcePause", vec![json!(gid)]);
                return Err(error);
            }
            if let Some(status) = child.0.try_wait()? {
                bail!("下载器意外退出（退出码：{status}）");
            }
            let status = rpc.call(
                "aria2.tellStatus",
                vec![
                    json!(gid),
                    json!([
                        "status",
                        "totalLength",
                        "completedLength",
                        "downloadSpeed",
                        "connections",
                        "errorCode",
                        "errorMessage"
                    ]),
                ],
            )?;
            match status["status"].as_str().context("下载器返回了无效状态")? {
                "complete" => return Ok(TransferOutcome::Complete),
                "error" if status["errorCode"] == "8" && connections > 1 => {
                    return Ok(TransferOutcome::RangeUnsupported);
                }
                "error" if status["errorCode"] == "32" => bail!(CHECKSUM_MISMATCH),
                "error" => bail!(
                    "aria2 下载失败（{}）：{}",
                    status["errorCode"].as_str().unwrap_or("?"),
                    status["errorMessage"].as_str().unwrap_or("未知错误")
                ),
                "removed" => bail!("下载任务已被移除"),
                _ => progress(parse_progress(&status)?),
            }
            thread::sleep(POLL_INTERVAL);
        }
    })();
    let _ = rpc.call("aria2.shutdown", vec![]);
    child.stop()?;
    #[cfg(windows)]
    drop(executable_lock);
    result
}

fn executable_path(embedded: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(embedded.to_path_buf())
    }
    #[cfg(not(windows))]
    {
        let _ = embedded;
        #[cfg(test)]
        {
            Ok(std::env::var_os("MIPCM_TEST_ARIA2")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/usr/bin/aria2c")))
        }
        #[cfg(not(test))]
        {
            bail!("内嵌下载器仅支持 Windows");
        }
    }
}

fn parse_progress(status: &Value) -> Result<DownloadProgress> {
    let number = |key: &str| -> Result<u64> {
        status[key]
            .as_str()
            .with_context(|| format!("缺少下载进度 {key}"))?
            .parse()
            .with_context(|| format!("无效下载进度 {key}"))
    };
    Ok(DownloadProgress {
        phase: DownloadPhase::Downloading,
        total: number("totalLength")?,
        completed: number("completedLength")?,
        bytes_per_second: number("downloadSpeed")?,
        connections: number("connections")?.try_into()?,
    })
}

struct ChildGuard(Child);

impl ChildGuard {
    fn stop(&mut self) -> Result<()> {
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if self.0.try_wait()?.is_some() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.0.kill().context("无法结束下载器")?;
        self.0.wait().context("无法等待下载器退出")?;
        Ok(())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// 仅与本次子进程的 loopback JSON-RPC 通信，不接触远程 HTTP 或代理。
struct RpcClient {
    address: SocketAddr,
    secret: String,
}

impl RpcClient {
    fn call(&self, method: &str, mut params: Vec<Value>) -> Result<Value> {
        params.insert(0, json!(format!("token:{}", self.secret)));
        let body = serde_json::to_vec(
            &json!({"jsonrpc": "2.0", "id": "mipcm", "method": method, "params": params}),
        )?;
        let retry_until = Instant::now() + Duration::from_secs(2);
        loop {
            match self.request(&body) {
                Err(error)
                    if method == "aria2.tellStatus"
                        && error.downcast_ref::<std::io::Error>().is_some()
                        && Instant::now() < retry_until =>
                {
                    // 只重试只读进度查询；addUri 等修改操作不能重发，避免创建重复任务。
                    thread::sleep(Duration::from_millis(100));
                }
                result => return result,
            }
        }
    }

    fn request(&self, body: &[u8]) -> Result<Value> {
        let timeout = Duration::from_secs(2);
        let mut stream = TcpStream::connect_timeout(&self.address, timeout)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        write!(
            stream,
            "POST /jsonrpc HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            body.len()
        )?;
        stream.write_all(body)?;
        let mut reader = BufReader::new(stream);
        let mut header = String::new();
        let mut length = None;
        loop {
            let mut line = String::new();
            if reader.by_ref().take(8192).read_line(&mut line)? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "下载器响应不完整",
                )
                .into());
            }
            header.push_str(&line);
            if header.len() > 8192 {
                bail!("下载器响应头过大");
            }
            if line == "\r\n" {
                break;
            }
            if let Some((key, value)) = line.split_once(':')
                && key.eq_ignore_ascii_case("content-length")
            {
                length = Some(value.trim().parse::<usize>()?);
            }
        }
        if !header.starts_with("HTTP/1.1 200 ") {
            bail!("下载器 RPC 请求失败");
        }
        let length = length
            .filter(|size| *size <= 65536)
            .context("下载器返回了无效响应长度")?;
        let mut response = vec![0; length];
        reader.read_exact(&mut response)?;
        let response: Value = serde_json::from_slice(&response)?;
        if !response["error"].is_null() {
            bail!("下载器 RPC 错误：{}", response["error"]);
        }
        if response["id"] != "mipcm" {
            bail!("下载器响应 ID 不匹配");
        }
        response
            .get("result")
            .cloned()
            .context("下载器响应缺少 result")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn verifies_embedded_payload_and_rejects_modified_files() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("aria2c.exe");
        unpack_payload(&executable).unwrap();
        assert_eq!(fs::metadata(&executable).unwrap().len(), 5_649_408);
        fs::write(&executable, b"modified executable").unwrap();
        assert!(verify_checksum(&executable, PAYLOAD_SHA256, &DownloadControl::default()).is_err());
    }

    #[test]
    fn rejects_overwrites_and_mismatched_resume_sources() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("setup.exe");
        fs::write(&target, b"existing file").unwrap();
        assert!(
            download(
                "https://example.com/setup.exe",
                &target,
                None,
                &DownloadControl::default(),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"existing file");
        fs::remove_file(&target).unwrap();
        fs::write(sidecar(&target, ".download.meta"), "another source").unwrap();
        fs::write(sidecar(&target, ".part"), b"partial file").unwrap();
        assert!(
            download(
                "https://example.com/setup.exe",
                &target,
                None,
                &DownloadControl::default(),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(
            fs::read(sidecar(&target, ".part")).unwrap(),
            b"partial file"
        );
    }

    #[test]
    fn download_lock_blocks_a_second_writer() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("setup.exe");
        let lock = File::create(sidecar(&target, ".download.lock")).unwrap();
        lock.lock().unwrap();
        assert!(
            download(
                "https://example.com/setup.exe",
                &target,
                None,
                &DownloadControl::default(),
                |_| {}
            )
            .is_err()
        );
        assert!(!sidecar(&target, ".download.meta").exists());
    }

    #[test]
    fn downloads_without_range_support_and_checks_integrity() {
        let fixture = HttpFixture::new(false, false);
        let dir = tempfile::Builder::new()
            .prefix("下载 ‘引号’ $() ` ")
            .tempdir()
            .unwrap();
        let target = dir.path().join("setup.exe");
        download(
            &fixture.url,
            &target,
            Some(&fixture.checksum),
            &DownloadControl::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(&target).unwrap(), *fixture.data);
        assert!(!sidecar(&target, ".part").exists());
        assert!(!sidecar(&target, ".download.meta").exists());
        // 已下载且符合固定指纹时直接复用，不再向服务器请求。
        let before = fixture.ranges.lock().unwrap().len();
        download(
            &fixture.url,
            &target,
            Some(&fixture.checksum),
            &DownloadControl::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(fixture.ranges.lock().unwrap().len(), before);
    }

    #[test]
    fn cancelled_segmented_download_resumes_with_real_progress() {
        let fixture = HttpFixture::new(true, true);
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("setup.exe");
        let control = DownloadControl::default();
        let mut completed = 0;
        let result = download(
            &fixture.url,
            &target,
            Some(&fixture.checksum),
            &control,
            |status| {
                if status.completed > 0 {
                    completed = status.completed;
                    control.cancel();
                }
            },
        );
        assert!(result.is_err());
        assert!(completed > 0 && completed < fixture.data.len() as u64);
        assert!(!target.exists());
        assert!(sidecar(&sidecar(&target, ".part"), ".aria2").is_file());
        fixture.ranges.lock().unwrap().clear();
        download(
            &fixture.url,
            &target,
            Some(&fixture.checksum),
            &DownloadControl::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(target).unwrap(), *fixture.data);
        let ranges = fixture.ranges.lock().unwrap();
        assert!(
            ranges.iter().any(|start| *start > 0),
            "必须请求未完成的字节范围"
        );
    }

    #[test]
    fn checksum_failure_never_promotes_a_download_to_exe() {
        let fixture = HttpFixture::new(false, false);
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("setup.exe");
        let result = download(
            &fixture.url,
            &target,
            Some(&"0".repeat(64)),
            &DownloadControl::default(),
            |_| {},
        );
        let error = result.unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("SHA-256 不匹配"), "{message}");
        assert!(sidecar(&target, ".part").is_file());
        assert!(!target.exists());
    }

    struct HttpFixture {
        url: String,
        checksum: String,
        data: Arc<Vec<u8>>,
        ranges: Arc<Mutex<Vec<usize>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl HttpFixture {
        fn new(ranged: bool, throttled: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/setup.exe", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let data = Arc::new(vec![b'X'; 16 * 1024 * 1024]);
            let checksum = format!("{:x}", Sha256::digest(&*data));
            let ranges = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let worker_data = data.clone();
            let worker_ranges = ranges.clone();
            let worker = thread::spawn(move || {
                let mut handlers = Vec::new();
                while !worker_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let data = worker_data.clone();
                            let ranges = worker_ranges.clone();
                            handlers.push(thread::spawn(move || {
                                let _ = serve(stream, &data, &ranges, ranged, throttled);
                            }));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(error) => panic!("{error}"),
                    }
                }
                for handler in handlers {
                    handler.join().unwrap();
                }
            });
            Self {
                url,
                checksum,
                data,
                ranges,
                stop,
                worker: Some(worker),
            }
        }
    }

    impl Drop for HttpFixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.worker.take().unwrap().join().unwrap();
        }
    }

    fn serve(
        mut stream: TcpStream,
        data: &[u8],
        ranges: &Mutex<Vec<usize>>,
        ranged: bool,
        throttled: bool,
    ) -> Result<()> {
        // Windows 的新连接继承监听 socket 的非阻塞模式；write_all 需要阻塞连接。
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut start = 0;
        let mut end = data.len() - 1;
        let mut partial = false;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if ranged && line.to_ascii_lowercase().starts_with("range: bytes=") {
                let value = line.split_once('=').unwrap().1.trim();
                let (first, last) = value.split_once('-').unwrap();
                start = first.parse()?;
                if !last.is_empty() {
                    end = last.parse::<usize>()?.min(end);
                }
                partial = true;
            }
        }
        ranges.lock().unwrap().push(start);
        if partial {
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\n",
                data.len()
            )?;
        } else {
            write!(stream, "HTTP/1.1 200 OK\r\n")?;
        }
        write!(
            stream,
            "Content-Length: {}\r\nAccept-Ranges: {}\r\nConnection: close\r\n\r\n",
            end - start + 1,
            if ranged { "bytes" } else { "none" }
        )?;
        for chunk in data[start..=end].chunks(32 * 1024) {
            stream.write_all(chunk)?;
            if throttled {
                thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(())
    }
}
