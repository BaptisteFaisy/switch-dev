import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const readRustModule = (path) => {
  try {
    return read(path);
  } catch (error) {
    if (!path.endsWith("src-tauri/src/voice.rs")) throw error;
    const moduleRoot = "../src-tauri/src/voice/";
    return [
      "mod.rs",
      "activity.rs",
      "audio.rs",
      "config.rs",
      "gpu.rs",
      "http.rs",
      "ollama.rs",
      "remote.rs",
      "whisper.rs",
    ].map((file) => read(`${moduleRoot}${file}`)).join("\n");
  }
};
const main = read("../src/main.ts");
const types = read("../src/main.ts");
const view = read("../src/transcription.ts");
const styles = read("../src/transcription.css");
const chatView = read("../src/chat/view.ts");
const chatVoice = read("../src/chat/voice.ts");
const platform = read("../src/platform.ts");
const backend = readRustModule("../src-tauri/src/voice.rs");
const server = read("../src-tauri/src/server.rs");
const rustLib = read("../src-tauri/src/lib.rs");
const dockerfile = read("../Dockerfile");
const gpuCompose = read("../compose.gpu.yaml");
const ansibleCompose = read("../deploy/ansible/templates/compose.yaml.j2");
const ansibleEnv = read("../deploy/ansible/templates/cst.env.j2");
const ansiblePlaybook = read("../deploy/ansible/playbook.yml");
const deploy = read("../scripts/deploy-vps-ansible.ps1");

test("un onglet Transcrire est charge a la demande sur desktop et mobile", () => {
  assert.match(types, /\| "transcription"/);
  assert.match(main, /import\("\.\/transcription"\)/);
  assert.match(main, /data-view="transcription"[^>]*>[\s\S]*?Transcrire/);
  assert.match(main, /id="transcriptionToggle"[\s\S]*?<span>Transcrire<\/span>/);
  assert.match(main, /case "transcription":[\s\S]*?renderTranscriptionPanel/);
  assert.match(main, /bindTranscriptionPanel\(render\)/);
});

test("la vue accepte un fichier, permet l'annulation et exporte le texte", () => {
  assert.match(view, /type="file"[^>]*accept="audio\/\*/);
  assert.match(view, /MAX_TRANSCRIPTION_AUDIO_BYTES/);
  assert.match(view, /dataTransfer\?\.files\?\.\[0\]/);
  assert.match(view, /abortController\?\.abort\(\)/);
  assert.match(view, /navigator\.clipboard\?\.writeText/);
  assert.match(view, /new Blob\(\[result\.text\]/);
  assert.match(view, /readonly[^>]*aria-label="Texte transcrit"/);
  assert.match(view, /value="faithful"/);
  assert.match(view, /value="clean"/);
  assert.match(view, /value="summary"/);
  assert.match(view, /transcribeAudioFile\(file, language, outputMode/);
  assert.match(styles, /@media \(max-width: 680px\)/);
  assert.match(styles, /prefers-reduced-motion/);
});

test("les icones du module charge a la demande existent dans le registre partage", () => {
  const registryStart = main.indexOf("const lucideIcons = {");
  const registryEnd = main.indexOf("};", registryStart);
  const registry = main.slice(registryStart, registryEnd);
  const iconNames = [...view.matchAll(/data-lucide="([a-z0-9-]+)"/g)]
    .map((match) => match[1]);
  for (const iconName of iconNames) {
    const component = iconName
      .split("-")
      .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
      .join("");
    assert.match(registry, new RegExp(`\\b${component}\\b`), `${iconName} n'est pas enregistre`);
  }
});

test("le fichier distant reste binaire et cible explicitement le VPS", () => {
  assert.match(platform, /export async function transcribeAudioFile/);
  assert.match(platform, /uploadRemoteAudioFile\(file, language, outputMode, signal\)/);
  assert.match(platform, /\/api\/transcriptions\?\$\{query\.toString\(\)\}/);
  assert.match(platform, /"Content-Type": file\.type \|\| "application\/octet-stream"/);
  assert.match(platform, /body: file/);
  assert.match(platform, /tauriInvoke<AudioFileTranscriptionResponse>\("transcribe_audio_file"/);
});

test("le backend propose les trois modes, segmente les longs audios et supprime ses fichiers temporaires", () => {
  assert.match(backend, /pub const MAX_AUDIO_FILE_BYTES: usize = 100 \* 1024 \* 1024/);
  assert.match(backend, /pub async fn transcribe_audio_file_bytes/);
  assert.match(backend, /transcribe_audio_remote/);
  assert.match(backend, /normalize_uploaded_audio_blocking/);
  assert.match(backend, /transcribe_audio_remote_chunked/);
  assert.match(backend, /REMOTE_AUDIO_CHUNK_SECONDS/);
  assert.match(backend, /\.arg\("pcm_s16le"\)/);
  assert.match(backend, /impl Drop for TempVoiceDir/);
  const start = backend.indexOf("pub async fn transcribe_audio_file_bytes");
  const end = backend.indexOf("pub async fn process_voice_request", start);
  const implementation = backend.slice(start, end);
  assert.match(implementation, /post_process_with_ollama/);
  assert.match(backend, /VoiceOutputMode::Faithful/);
  assert.match(backend, /VoiceOutputMode::Clean/);
  assert.match(backend, /VoiceOutputMode::Summary/);
  assert.match(rustLib, /voice::transcribe_audio_file/);
});

test("le micro reutilise le meme choix de sortie et le memorise", () => {
  assert.match(chatView, /data-chat-control="voice-mode"/);
  assert.match(chatView, /value="clean" selected/);
  assert.match(chatVoice, /VOICE_OUTPUT_MODE_STORAGE_KEY/);
  assert.match(chatVoice, /outputMode: session\.outputMode/);
  assert.match(platform, /outputMode: args\.outputMode \?\? "clean"/);
});

test("cst-server protege et borne la route de transcription", () => {
  assert.match(server, /"\/transcriptions"/);
  assert.match(server, /DefaultBodyLimit::max\(voice::MAX_AUDIO_FILE_BYTES\)/);
  assert.match(server, /api_transcribe_audio_file[\s\S]*?check_admin_header/);
  assert.match(server, /body: Bytes/);
  assert.match(server, /voice::transcribe_audio_file_bytes/);
});

test("le deploiement GPU garde Speaches prive et verifie CUDA", () => {
  assert.match(dockerfile, /ffmpeg/);
  assert.match(gpuCompose, /ghcr\.io\/speaches-ai\/speaches:latest-cuda-12\.6\.3/);
  assert.match(gpuCompose, /CST_VOICE_TRANSCRIPTION_URL: http:\/\/speaches:8000\/v1\/audio\/transcriptions/);
  assert.match(gpuCompose, /capabilities: \[gpu\]/);
  assert.doesNotMatch(gpuCompose, /ports:/);
  assert.match(ansibleCompose, /cst_gpu_transcription/);
  assert.match(ansibleEnv, /CST_VOICE_TRANSCRIPTION_ACCELERATOR="gpu"/);
  assert.match(ansiblePlaybook, /nvidia-ctk, runtime, configure, --runtime=docker/);
  assert.match(ansiblePlaybook, /nvidia\/cuda:12\.6\.3-base-ubuntu24\.04/);
  assert.match(deploy, /\[switch\]\$GpuTranscription/);
  assert.match(deploy, /\[string\]\$TranscriptionUrl/);
  assert.match(deploy, /CST_VOICE_TRANSCRIPTION_URL"\] = \$TranscriptionUrl\.Trim\(\)/);
  assert.match(deploy, /transcriptionAccelerator = \$TranscriptionAccelerator/);
  assert.match(deploy, /cst_transcription_model = \$TranscriptionModel\.Trim\(\)/);
  assert.match(deploy, /gpuTranscription = \[bool\]\$GpuTranscription/);
});
