// Lector's window.
//
// No framework and no build step: the whole UI is one file of DOM calls, which
// keeps Node out of CI entirely. The window holds no state of its own beyond
// what is being typed -- every change re-reads a snapshot from Rust, so the two
// can never disagree.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
};

let snap = null;
let filter = "";
const open = new Set();          // which model rows are expanded
const progress = new Map();      // model id -> {received, total, phase}

async function refresh() {
  snap = await invoke("snapshot");
  render();
}

function voiceLabel(model, sp) {
  // Single-speaker files have no speaker name of their own; the model's label
  // is the voice's name in that case.
  return sp.name || model.label;
}

function render() {
  if (!snap) return;

  $("count").textContent = `${snap.voice_count} voices`;

  const chosen = snap.models.find((m) => m.id === snap.model_id);
  const sp = chosen?.speakers.find((s) => s.sid === snap.speaker);
  $("now").innerHTML = "";
  if (chosen && sp) {
    $("now").append(voiceLabel(chosen, sp), el("small", null, `  ·  ${chosen.accent}`));
  } else {
    $("now").append(snap.ready ? "Ready" : "No voice installed");
  }

  // Warnings earn the top of the window only when they are actionable.
  const warn = $("warn");
  if (!snap.accessibility) {
    $("warntext").textContent =
      "Lector needs Accessibility permission to read the text you select in other apps.";
    $("warnbtn").style.display = "";
    warn.classList.add("show");
  } else if (!snap.ready) {
    $("warntext").textContent = "Download a voice to get started — Kitten Nano is the quickest.";
    $("warnbtn").style.display = "none";
    warn.classList.add("show");
  } else {
    warn.classList.remove("show");
  }

  $("go").textContent = snap.speaking ? "Stop" : "Speak";
  $("go").classList.toggle("stop", snap.speaking);
  $("go").disabled = !snap.ready;

  $("speed").value = snap.speed;
  $("speedval").textContent = `${Number(snap.speed).toFixed(2).replace(/0$/, "")}×`;

  renderModels();
}

function matches(model, sp) {
  if (!filter) return true;
  const hay = `${model.label} ${model.accent} ${sp ? sp.name + " " + sp.note : ""}`.toLowerCase();
  return hay.includes(filter);
}

function renderModels() {
  const box = $("models");
  box.innerHTML = "";

  let shown = 0;
  for (const m of snap.models) {
    const hits = m.speakers.filter((sp) => matches(m, sp));
    if (!matches(m, null) && hits.length === 0) continue;
    shown++;

    const node = el("div", "model");
    // A search should show what it found, so matching rows open themselves.
    if (open.has(m.id) || (filter && hits.length)) node.classList.add("open");

    const head = el("div", "head");
    head.append(el("span", "chev", "▶"), el("span", "name", m.label));

    if (m.installing) {
      const p = progress.get(m.id);
      head.append(el("span", "meta", p?.phase === "Downloading" ? "downloading" : (p?.phase ?? "…").toLowerCase()));
      node.append(head);
      const bar = el("div", "bar");
      const fill = el("i");
      if (p?.total) fill.style.width = `${(p.received / p.total) * 100}%`;
      bar.append(fill);
      node.append(bar);
      if (p?.total) {
        node.append(el("div", "phase", `${Math.round((p.received / p.total) * 100)}% of ${m.mb} MB`));
      }
    } else if (!m.installed) {
      head.append(el("span", "meta", `${m.speakers.length > 1 ? m.speakers.length + " voices · " : ""}${m.mb} MB`));
      const btn = el("button", "get", "Get");
      btn.onclick = (e) => {
        e.stopPropagation();
        invoke("install", { modelId: m.id });
      };
      head.append(btn);
      head.title = m.tradeoff;
      node.append(head);
    } else {
      head.append(el("span", "meta", m.speakers.length > 1 ? `${m.speakers.length} voices` : m.accent));
      head.onclick = () => {
        open.has(m.id) ? open.delete(m.id) : open.add(m.id);
        renderModels();
      };
      node.append(head);

      const list = el("div", "voices");
      for (const sp of hits) {
        const row = el("div", "voice");
        if (m.id === snap.model_id && sp.sid === snap.speaker) row.classList.add("on");
        row.append(el("span", null, voiceLabel(m, sp)));
        if (sp.note) row.append(el("span", "note", sp.note));

        const play = el("button", "play", "▶");
        play.title = "Hear this voice";
        play.onclick = (e) => {
          e.stopPropagation();
          invoke("audition", { modelId: m.id, sid: sp.sid });
        };
        row.append(play);

        row.onclick = () => invoke("choose_voice", { modelId: m.id, sid: sp.sid });
        list.append(row);
      }
      node.append(list);
    }
    box.append(node);
  }

  if (!shown) box.append(el("div", "empty", "Nothing matches that."));
}

// ---- events ----

$("search").oninput = (e) => {
  filter = e.target.value.trim().toLowerCase();
  renderModels();
};

$("go").onclick = () => {
  if (snap?.speaking) return invoke("stop");
  const text = $("text").value.trim();
  if (text) invoke("speak", { text });
};

$("speed").oninput = (e) => {
  $("speedval").textContent = `${Number(e.target.value).toFixed(2).replace(/0$/, "")}×`;
};
$("speed").onchange = (e) => invoke("set_speed", { speed: Number(e.target.value) });

$("warnbtn").onclick = () => invoke("grant_accessibility");

// Cmd+Enter speaks, which is the shortcut people try first in a text box.
$("text").onkeydown = (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
    e.preventDefault();
    $("go").click();
  }
};

listen("changed", refresh);
listen("progress", (e) => {
  progress.set(e.payload.id, e.payload);
  if (e.payload.phase === "Done") {
    progress.delete(e.payload.id);
    refresh();
  } else {
    renderModels();
  }
});
listen("failed", (e) => {
  progress.delete(e.payload.id);
  refresh();
  $("warntext").textContent = e.payload.error;
  $("warnbtn").style.display = "none";
  $("warn").classList.add("show");
});

// Speaking state is polled rather than pushed: it changes far faster than
// anything worth an event, and the window is often not visible.
setInterval(async () => {
  if (document.hidden) return;
  const s = await invoke("snapshot");
  if (s.speaking !== snap?.speaking || s.ready !== snap?.ready) {
    snap = s;
    render();
  }
}, 400);

// The level meter on the pane's top edge. Polled far more often than the
// snapshot, and only while there is something to show: a meter that updates
// four times a second looks broken, and one that polls while idle is waste.
const levelBar = $("level").firstElementChild;
setInterval(async () => {
  if (document.hidden || !snap?.speaking) {
    levelBar.style.width = "0";
    return;
  }
  const v = await invoke("level");
  // Square-root the amplitude: speech spends most of its time well below peak,
  // and a linear meter barely moves.
  levelBar.style.width = `${Math.min(1, Math.sqrt(v)) * 100}%`;
}, 60);

refresh();
