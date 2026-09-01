//! Statut GPU local interroge via nvidia-smi (accelerateur de transcription).

use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceGpuStatus {
    pub index: u32,
    pub name: String,
    pub utilization_percent: u32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
}

pub(crate) fn parse_nvidia_gpu_csv(output: &str) -> Option<VoiceGpuStatus> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    let values = line.split(',').map(str::trim).collect::<Vec<_>>();
    if values.len() != 5 {
        return None;
    }
    Some(VoiceGpuStatus {
        index: values[0].parse().ok()?,
        name: values[1].to_string(),
        utilization_percent: values[2].parse().ok()?,
        memory_used_mb: values[3].parse().ok()?,
        memory_total_mb: values[4].parse().ok()?,
    })
}

fn query_nvidia_gpu() -> Option<VoiceGpuStatus> {
    let mut command = Command::new("nvidia-smi");
    command.args([
        "--query-gpu=index,name,utilization.gpu,memory.used,memory.total",
        "--format=csv,noheader,nounits",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
        .and_then(|stdout| parse_nvidia_gpu_csv(&stdout))
}

pub(crate) async fn query_gpu_async() -> Option<VoiceGpuStatus> {
    tokio::task::spawn_blocking(query_nvidia_gpu)
        .await
        .unwrap_or(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_status_parses_nvidia_smi_csv() {
        let status =
            parse_nvidia_gpu_csv("0, NVIDIA GeForce RTX 3060 Ti, 37, 4276, 8192\n").unwrap();
        assert_eq!(status.index, 0);
        assert_eq!(status.name, "NVIDIA GeForce RTX 3060 Ti");
        assert_eq!(status.utilization_percent, 37);
        assert_eq!(status.memory_used_mb, 4276);
        assert_eq!(status.memory_total_mb, 8192);
    }
}
