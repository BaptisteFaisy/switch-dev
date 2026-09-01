import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const panel = readFileSync(new URL("../src/video.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/video.css", import.meta.url), "utf8");
const skillsIndex = readFileSync(new URL("../public/skills/index.json", import.meta.url), "utf8");
const carrouselSkill = readFileSync(new URL("../public/skills/carrousel/SKILL.md", import.meta.url), "utf8");

test("le Studio IA propose un onglet Carrousel avec son panneau complet", () => {
  assert.match(panel, /data-creative-kind="carousel"/);
  assert.match(panel, /renderCarouselStudioPanel/);
  assert.match(panel, /export const renderVideoPanel = \(\): string => \{[\s\S]*?creativeKind === "carousel"[\s\S]*?renderCarouselStudioPanel\(\)/);
  assert.match(panel, /id="carouselForm"/);
  assert.match(panel, /id="carouselName"/);
  assert.match(panel, /id="carouselDropzone"/);
  assert.match(panel, /id="carouselImageFile"[^>]*multiple/);
  assert.match(panel, /id="carouselSave"/);
  assert.match(panel, /id="carouselNew"/);
  assert.match(panel, /id="carouselPrev"/);
  assert.match(panel, /id="carouselNext"/);
  assert.match(panel, /Carrousels enregistrés/);
});

test("l'import d'images restreint aux formats joignables dans les chats", () => {
  assert.match(panel, /accept="image\/png,image\/jpeg,image\/webp"/);
  assert.match(panel, /image\\\/\(\?:png\|jpeg\|webp\)/);
  assert.match(panel, /CAROUSEL_MAX_IMAGE_BYTES/);
});

test("chaque diapositive porte sa data : titre, texte, bouton et lien", () => {
  assert.match(panel, /data-carousel-field="title"/);
  assert.match(panel, /data-carousel-field="caption"/);
  assert.match(panel, /data-carousel-field="ctaLabel"/);
  assert.match(panel, /data-carousel-field="ctaUrl"/);
  assert.match(panel, /data-carousel-move=/);
  assert.match(panel, /data-carousel-remove/);
  assert.match(panel, /data-carousel-dot=/);
});

test("les carrousels sont enregistres localement avec normalisation et limites", () => {
  assert.match(panel, /codex-switch-terminal\.carousels\.v1/);
  assert.match(panel, /export const normalizeCarousels/);
  assert.match(panel, /export function loadCarousels/);
  assert.match(panel, /export function persistCarousels/);
  assert.match(panel, /CAROUSEL_LIMIT = 12/);
  assert.match(panel, /CAROUSEL_SLIDE_LIMIT = 10/);
  assert.match(panel, /normalizeCarousels\(items\)\.slice\(0, CAROUSEL_LIMIT\)/);
});

test("le Studio IA envoie le carrousel aux chats (data + images)", () => {
  assert.match(panel, /switch:carousel-send-to-chat/);
  assert.match(panel, /window\.dispatchEvent\(new CustomEvent<CarouselChatPayload>\(CAROUSEL_SEND_TO_CHAT_EVENT/);
  assert.match(main, /const CAROUSEL_SEND_TO_CHAT_EVENT = "switch:carousel-send-to-chat"/);
  assert.match(main, /window\.addEventListener\(CAROUSEL_SEND_TO_CHAT_EVENT/);
  assert.match(main, /carouselSendToChatBound/);
  assert.match(main, /pane\.imageAttachments = \[\.\.\.pane\.imageAttachments, \.\.\.attached\]/);
  assert.match(main, /MAX_CHAT_IMAGE_ATTACHMENTS/);
  assert.match(main, /carouselPromptForChat/);
  assert.match(main, /Diapositive \$\{index \+ 1\}/);
});

test("le skill Carrousel est embarque pour que les chats disposent de l'outil", () => {
  assert.match(skillsIndex, /"id": "carrousel"/);
  assert.match(skillsIndex, /"file": "carrousel\/SKILL\.md"/);
  assert.match(skillsIndex, /"buttonLabel": "Carrousel"/);
  assert.match(skillsIndex, /"icon": "layout-grid"/);
  assert.match(carrouselSkill, /# Carrousel/);
  assert.match(carrouselSkill, /Studio IA/);
  assert.match(carrouselSkill, /appel à l'action|appel à l’action/);
});

test("le studio carrousel est style, responsive et sans regressions video", () => {
  assert.match(styles, /\.carousel-preview-frame\s*\{/);
  assert.match(styles, /\.carousel-slide-editor\s*\{/);
  assert.match(styles, /\.carousel-dropzone\s*\{/);
  assert.match(styles, /\.carousel-saved-item\s*\{/);
  assert.match(styles, /\.creative-carousel-hint\s*\{/);
  assert.match(styles, /@media \(max-width: 1120px\)/);
  assert.match(styles, /@media \(max-width: 760px\)/);
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)/);
  assert.match(styles, /repeat\(auto-fit, minmax\(104px, 1fr\)\)/);
  assert.match(panel, /data-creative-kind="video"/);
  assert.match(panel, /data-creative-kind="image"/);
});
