import { parseLanguage, highlightedHtml } from "./highlight";
import { openModal } from "../modal";

function singleFencedBlock(s: string): { code: string; language: string } | null {
  const trimmed = s.trim();
  if (!trimmed.startsWith("```") || !trimmed.endsWith("```")) return null;
  const parts = trimmed.split("```");
  if (parts.length !== 3) return null;
  let lines = trimmed.split("\n");
  const language = lines[0].slice(3).trim();
  if (lines[lines.length - 1].trim() === "```") lines = lines.slice(0, -1);
  lines = lines.slice(1);
  return { code: lines.join("\n"), language: language || "text" };
}

function prettyJsonMaybe(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return raw;
  if (raw.includes("```")) return raw;
  if (trimmed[0] !== "{" && trimmed[0] !== "[") return raw;
  try {
    const obj = JSON.parse(trimmed);
    return "```json\n" + JSON.stringify(obj, Object.keys(obj).sort(), 2) + "\n```";
  } catch { return raw; }
}

export function formatResponse(raw: string): string {
  return prettyJsonMaybe(raw);
}

export function createResponseViewer(): { open: (title: string, text: string) => void } {
  let closeActive: (() => void) | null = null;

  const getFontSize = () => Number(localStorage.getItem("responseFontSize") ?? "13");
  const getLineHeight = () => Number(localStorage.getItem("responseLineHeight") ?? "4");
  const setFontSize = (v: number) => localStorage.setItem("responseFontSize", String(v));
  const setLineHeight = (v: number) => localStorage.setItem("responseLineHeight", String(v));

  function open(title: string, text: string) {
    closeActive?.();
    const display = formatResponse(text);
    const block = singleFencedBlock(display);
    const code = block?.code ?? display;
    const lang = block ? parseLanguage(block.language) : "text";

    const content = document.createElement("div");
    content.className = "response-viewer";
    content.innerHTML = `<div class="response-viewer-body"><div class="response-viewer-gutter" data-role="gutter"></div><pre class="response-viewer-code" data-role="code"></pre></div>`;
    const controls = document.createElement("div");
    controls.className = "response-viewer-controls";
     controls.innerHTML = `<div class="rv-stepper" aria-label="Text size"><span>Size</span><button data-action="font-dec" aria-label="Decrease text size">-</button><span class="rv-value" data-role="fontVal">${getFontSize()}</span><button data-action="font-inc" aria-label="Increase text size">+</button></div><div class="rv-stepper" aria-label="Line spacing"><span>Spacing</span><button data-action="lh-dec" aria-label="Decrease line spacing">-</button><span class="rv-value" data-role="lhVal">${getLineHeight()}</span><button data-action="lh-inc" aria-label="Increase line spacing">+</button></div><button class="rv-btn" data-action="copy">Copy</button>`;
    const modal = openModal({ title: `Response: ${title}`, size: "large", content, headerActions: controls, onClose: () => { closeActive = null; } });
    closeActive = modal.close;
    const body = content.querySelector("[data-role='code']") as HTMLElement;
    const gutter = content.querySelector("[data-role='gutter']") as HTMLElement;

    const render = async () => {
      if (block) {
        const html = await highlightedHtmlAsync(code, lang);
        // line numbers
        const lines = code.split("\n");
        gutter.textContent = lines.map((_, i) => String(i + 1)).join("\n");
        body.innerHTML = html;
      } else {
        gutter.textContent = "";
        // Markdown-ish prose: just show as monospace for now
        body.textContent = code;
      }
      const fs = getFontSize();
      const lh = getLineHeight();
      body.style.fontSize = `${fs}px`;
      body.style.lineHeight = `${fs + lh}px`;
      gutter.style.fontSize = `${fs}px`;
      gutter.style.lineHeight = `${fs + lh}px`;
      controls.querySelector("[data-role='fontVal']")!.textContent = String(fs);
      controls.querySelector("[data-role='lhVal']")!.textContent = String(lh);
    };
    render();

    controls.querySelector("[data-action='copy']")?.addEventListener("click", async () => {
      await navigator.clipboard.writeText(text);
      const btn = controls.querySelector("[data-action='copy']") as HTMLElement;
      btn.textContent = "Copied!";
      setTimeout(() => (btn.textContent = "Copy"), 1200);
    });
    const upd = (deltaFont: number, deltaLh: number) => {
      const fs = Math.max(10, Math.min(24, getFontSize() + deltaFont));
      const lh = Math.max(0, Math.min(14, getLineHeight() + deltaLh));
      setFontSize(fs); setLineHeight(lh); render();
    };
    controls.querySelector("[data-action='font-inc']")?.addEventListener("click", () => upd(1,0));
    controls.querySelector("[data-action='font-dec']")?.addEventListener("click", () => upd(-1,0));
    controls.querySelector("[data-action='lh-inc']")?.addEventListener("click", () => upd(0,1));
    controls.querySelector("[data-action='lh-dec']")?.addEventListener("click", () => upd(0,-1));
  }
  return { open };
}

async function highlightedHtmlAsync(source: string, lang: ReturnType<typeof parseLanguage>) {
  // off-interaction-path: yield before heavy highlight
  if (source.length > 4000) await new Promise(r => setTimeout(r, 0));
  return highlightedHtml(source, lang);
}
