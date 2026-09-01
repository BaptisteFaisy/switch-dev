import { execSync } from "node:child_process";
import { defineConfig, type Plugin } from "vite";

const buildId =
  process.env.CST_BUILD_ID
  ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;

// Commit git embarque dans le bundle frontend pour la tracabilite du build.
// L'auto-update utilise le buildId propre au frontend dans index.html : une
// publication CSS/JS peut ainsi rester independante du commit du backend.
// Priorite identique a src-tauri/build.rs : env CST_GIT_COMMIT, puis
// `git rev-parse --short HEAD`, puis "unknown".
const gitShortCommit = (): string | undefined => {
  try {
    const commit = execSync("git rev-parse --short HEAD", { encoding: "utf8" }).trim();
    return commit || undefined;
  } catch {
    return undefined;
  }
};

const buildCommit = process.env.CST_GIT_COMMIT?.trim() || gitShortCommit() || "unknown";

// Le chunk d'entree est charge via un import() dynamique plutot qu'un
// <script type="module" src="..."> statique. Si un index.html perime (garde en
// cache par le service worker) reference un chunk deja purge du serveur, le
// navigateur rejette l'import : on recharge une seule fois avec
// ?cst-chunk-build=<buildId> pour repartir d'un index frais, au lieu de laisser
// le boot-splash tourner indefiniment sans message d'erreur.
const dynamicEntryChunk = (): Plugin => ({
  name: "cst-dynamic-entry-chunk",
  enforce: "post",
  apply: "build",
  transformIndexHtml(html) {
    const entry = /<script type="module" crossorigin src="(\/assets\/index-[^"]+\.js)"><\/script>/.exec(html);
    if (!entry) return html;
    const entryUrl = entry[1];
    const bootstrap = [
      '<script type="module">',
      "const u = new URL(window.location.href);",
      `import(${JSON.stringify(entryUrl)}).catch(() => {`,
      `  if (u.searchParams.get("cst-chunk-build") !== ${JSON.stringify(buildId)}) {`,
      `    u.searchParams.set("cst-chunk-build", ${JSON.stringify(buildId)});`,
      "    window.location.replace(u.toString());",
      "  }",
      "});",
      "</script>",
    ].join("");
    return {
      html: html.replace(entry[0], bootstrap),
      tags: [
        {
          tag: "meta",
          attrs: { name: "cst-build-id", content: buildId },
          injectTo: "head-prepend",
        },
        {
          tag: "meta",
          attrs: { name: "cst-build-commit", content: buildCommit },
          injectTo: "head-prepend",
        },
      ],
    };
  },
});

export default defineConfig({
  plugins: [dynamicEntryChunk()],
  clearScreen: false,
  define: {
    __CST_BUILD_ID__: JSON.stringify(buildId),
    __CST_BUILD_COMMIT__: JSON.stringify(buildCommit),
  },
  build: {
    emptyOutDir: true,
    target: "es2022",
    modulePreload: { polyfill: false },
  },
  server: {
    strictPort: true,
    host: "127.0.0.1",
    port: 1420,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});

