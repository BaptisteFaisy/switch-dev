import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { openExternalHttpsUrl } from "./platform";
import { terminalThemeFor, type ThemeMode } from "./theme";

/**
 * Le moteur xterm est volontairement isole dans ce module. `main.ts` le charge
 * avec import() uniquement lorsqu'un terminal est effectivement ouvert ou
 * restaure : les utilisateurs du chat ne paient ni son transfert, ni son parse.
 */
export const createTerminalRuntime = (theme: ThemeMode = "dark") => {
  const terminal = new Terminal({
    cursorBlink: true,
    cursorStyle: "bar",
    fontFamily: "Cascadia Mono, Consolas, monospace",
    fontSize: 13,
    lineHeight: 1.15,
    // Vingt terminaux peuvent rester montes sans conserver 20 historiques de
    // mille lignes. Le processus PTY reste actif ; seule la memoire d'affichage
    // ancienne est bornee.
    scrollback: 500,
    // Les CLI lances sur un VPS ne peuvent pas ouvrir le navigateur local.
    // Relayer leurs liens OSC 8 via la couche native permet notamment a
    // `claude auth login` d'ouvrir OAuth dans le navigateur du poste client.
    linkHandler: {
      activate: (_event, uri) => {
        void openExternalHttpsUrl(uri).catch((error) => {
          terminal.writeln(`\r\nImpossible d'ouvrir ce lien : ${String(error)}\r\n`);
        });
      },
    },
    theme: terminalThemeFor(theme),
  });
  const fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  return { terminal, fitAddon };
};
