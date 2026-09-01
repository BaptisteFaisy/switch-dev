import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const panel = readFileSync(new URL("../src/video.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/video.css", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const desktop = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/video_generation.rs", import.meta.url), "utf8");
const imageBackend = readFileSync(new URL("../src-tauri/src/image_generation.rs", import.meta.url), "utf8");
const accountsBackend = readFileSync(new URL("../src-tauri/src/creative_accounts.rs", import.meta.url), "utf8");

test("le studio video est charge a la demande sur bureau et mobile", () => {
  assert.match(types, /\| "video"/);
  assert.match(main, /id="videoToggle"/);
  assert.match(main, /data-view="video"/);
  assert.match(main, /type VideoModule = typeof import\("\.\/video"\)/);
  assert.match(main, /videoModulePromise = import\("\.\/video"\)/);
  assert.match(main, /if \(view === "video" && !videoModule\)/);
  assert.match(main, /case "video":\s*return videoModule\?\.renderVideoPanel\(\) \?\? ""/);
  assert.match(main, /videoModule\?\.activateVideoPanel\(render\)/);
  assert.doesNotMatch(main, /from "\.\/video";/);
  assert.match(panel, /import "\.\/video\.css";/);
});

test("le studio couvre texte-vers-video, image-vers-video et les options essentielles", () => {
  assert.match(panel, /data-creative-kind="video"/);
  assert.match(panel, /data-creative-kind="image"/);
  assert.match(panel, /data-video-mode="text"/);
  assert.match(panel, /data-video-mode="image"/);
  assert.match(panel, /id="videoImageFile"/);
  assert.match(panel, /id="videoPrompt"/);
  assert.match(panel, /id="videoAspectRatio"/);
  assert.match(panel, /id="videoDuration"/);
  assert.match(panel, /id="videoResolution"/);
  assert.match(panel, /id="videoGenerateAudio"/);
  assert.match(panel, /id="videoNegativePrompt"/);
  assert.match(panel, /imageDefinesAspectRatio/);
  assert.match(panel, /Le format est d[Ã©é]termin[Ã©é] par l.image de d[Ã©é]part/);
  assert.match(panel, /<video[^>]+controls/);
});

test("le studio genere aussi des images avec trois moteurs et un historique separe", () => {
  assert.match(panel, /id="imageGenerationForm"/);
  assert.match(panel, /id="imagePrompt"/);
  assert.match(panel, /id="imageSize"/);
  assert.match(panel, /id="imageStyle"/);
  assert.match(panel, /id="imageCount"/);
  assert.match(panel, /invoke<ImageBackendJob>\("start_image_generation"/);
  assert.match(panel, /invoke<ImageBackendJob>\("image_generation_status"/);
  assert.match(panel, /invoke<ImageBackendJob>\("cancel_image_generation"/);
  assert.match(panel, /codex-switch-terminal\.image-generations\.v1/);
  for (const id of ["flux-2-flash", "ideogram-v3", "recraft-v3"]) {
    assert.match(imageBackend, new RegExp(`id: "${id.replaceAll("-", "\\-")}"`));
  }
});

test("plusieurs comptes fal peuvent etre connectes, selectionnes et deconnectes", () => {
  assert.match(panel, /id="creativeAccountSelect"/);
  assert.match(panel, /id="creativeAccountForm"/);
  assert.match(panel, /id="creativeAccountKey"[^>]+type="password"/);
  assert.match(panel, /invoke<CreativeAccountsView>\("connect_creative_account"/);
  assert.match(panel, /invoke<CreativeAccountsView>\("set_default_creative_account"/);
  assert.match(panel, /invoke<CreativeAccountsView>\("delete_creative_account"/);
  assert.match(panel, /accountId: account\.id/);
  assert.match(accountsBackend, /creative-accounts\.json/);
  assert.match(accountsBackend, /validate_fal_key_remotely/);
  assert.match(accountsBackend, /FAL_API_BASE_URL: &str = "https:\/\/api\.fal\.ai"/);
  assert.match(accountsBackend, /\/v1\/models\?limit=1/);
});

test("les generations asynchrones sont suivies, annulables et memorisees localement", () => {
  assert.match(panel, /invoke<VideoBackendJob>\("start_video_generation"/);
  assert.match(panel, /invoke<VideoBackendJob>\("video_generation_status"/);
  assert.match(panel, /invoke<VideoBackendJob>\("cancel_video_generation"/);
  assert.match(panel, /window\.setTimeout\(\(\) => void pollJobs\(\), delay\)/);
  assert.match(panel, /codex-switch-terminal\.video-generations\.v1/);
  assert.match(panel, /persistVideoHistory\(jobs\)/);
  assert.match(panel, /Cr[Ã©é]ations r[Ã©é]centes/);
});

test("le client web utilise exclusivement les routes creatives authentifiees", () => {
  assert.match(platform, /case "video_generation_capabilities":\s*return api<T>\("GET", "\/api\/video\/capabilities"\)/);
  assert.match(platform, /case "start_video_generation":\s*return api<T>\("POST", "\/api\/video\/generations", args\.request\)/);
  assert.match(platform, /case "video_generation_status":\s*return api<T>\("POST", "\/api\/video\/generations\/status", args\.request\)/);
  assert.match(platform, /case "cancel_video_generation":\s*return api<T>\("POST", "\/api\/video\/generations\/cancel", args\.request\)/);
  assert.match(platform, /case "creative_accounts":\s*return api<T>\("GET", "\/api\/creative\/accounts"\)/);
  assert.match(platform, /case "start_image_generation":\s*return api<T>\("POST", "\/api\/image\/generations", args\.request\)/);
  assert.match(platform, /case "image_generation_status":\s*return api<T>\("POST", "\/api\/image\/generations\/status", args\.request\)/);
  assert.match(platform, /case "cancel_image_generation":\s*return api<T>\("POST", "\/api\/image\/generations\/cancel", args\.request\)/);

  for (const handler of [
    "api_creative_accounts",
    "api_start_image_generation",
    "api_image_generation_status",
    "api_cancel_image_generation",
    "api_video_generation_capabilities",
    "api_start_video_generation",
    "api_video_generation_status",
    "api_cancel_video_generation",
  ]) {
    const start = server.indexOf(`async fn ${handler}`);
    assert.notEqual(start, -1, `${handler} absent`);
    assert.match(server.slice(start, start + 800), /creative_owner_id\(&state, &headers\)/);
  }
});

test("la cle fal reste cote Rust et les destinations distantes sont en liste blanche", () => {
  assert.match(accountsBackend, /\["CST_FAL_KEY", "FAL_KEY"\]/);
  assert.match(accountsBackend, /pub struct CreativeAccountView/);
  assert.doesNotMatch(accountsBackend.slice(
    accountsBackend.indexOf("pub struct CreativeAccountView"),
    accountsBackend.indexOf("pub struct CreativeAccountsView"),
  ), /api_key/);
  assert.match(backend, /FAL_QUEUE_BASE_URL: &str = "https:\/\/queue\.fal\.run"/);
  assert.match(imageBackend, /https:\/\/queue\.fal\.run\/\{\}/);
  assert.match(backend, /const VIDEO_MODELS: &\[VideoModelSpec\]/);
  for (const id of ["wan-2.6", "veo-3.1-fast", "kling-3-standard", "luma-ray-2"]) {
    assert.match(backend, new RegExp(`id: "${id.replaceAll("-", "\\-")}"`));
  }
  assert.match(backend, /validate_request_id/);
  assert.match(backend, /Policy::none\(\)/);
  assert.match(backend, /état de génération vide ou absent/);
  assert.match(backend, /état de génération inconnu/);
  assert.doesNotMatch(panel, /CST_FAL_KEY|FAL_KEY|Authorization|Key \$\{/);
  assert.match(accountsBackend, /Permissions::from_mode\(0o600\)/);
  assert.match(desktop, /creative_accounts::connect_creative_account/);
  assert.match(desktop, /image_generation::start_image_generation/);
  assert.match(desktop, /video_generation::start_video_generation/);
});

test("le studio video est responsive et respecte la reduction des animations", () => {
  assert.match(styles, /@media \(max-width: 1120px\)/);
  assert.match(styles, /@media \(max-width: 760px\)/);
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)/);
  assert.match(styles, /\.video-studio-layout\s*\{/);
  assert.match(styles, /\.creative-account-overlay\s*\{/);
  assert.match(styles, /\.creative-image-gallery\s*\{/);
});
