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
// The book currently opened in the main pane, as returned by open_book. Null
// means the pane is showing pasted text, which is also where the app starts.
let book = null;
let shelf = [];
const covers = new Map();   // book id -> data URL, fetched once
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

  $("pause").hidden = !snap.speaking;
  $("pause").textContent = snap.paused ? "Resume" : "Pause";

  // Speaking finished on its own: go back to whatever the pane was showing.
  if (!snap.speaking && !advancing && !$("follow").hidden) hideFollow();

  $("speed").value = snap.speed;
  $("speedval").textContent = `${Number(snap.speed).toFixed(2).replace(/0$/, "")}×`;

  renderModels();
}

function matches(model, sp) {
  if (!filter) return true;
  const hay = `${model.label} ${model.accent} ${sp ? sp.name + " " + sp.note : ""}`.toLowerCase();
  return hay.includes(filter);
}

// The voice picker.
//
// Cards, not a tree. Two rules from verba, which learned both the hard way:
// name the tradeoff rather than the model, because "Noticeably better, a
// larger download" is a choice and "Kokoro v1.0 349 MB" is a quiz; and make
// exactly one recommendation, because two recommendations is a list with no
// recommendation in it. Everything else sorts by size ascending, since
// somebody ignoring the badge is usually looking for the cheapest download.

function orderedModels() {
  return [...snap.models].sort((a, b) => {
    const active = (m) => (m.id === snap.model_id ? 0 : 1);
    if (active(a) !== active(b)) return active(a) - active(b);
    if (a.installed !== b.installed) return a.installed ? -1 : 1;
    if (a.recommended !== b.recommended) return a.recommended ? -1 : 1;
    return a.mb - b.mb;
  });
}

function renderModels() {
  const box = $("models");
  box.innerHTML = "";

  let shown = 0;
  for (const m of orderedModels()) {
    const hits = m.speakers.filter((sp) => matches(m, sp));
    if (!matches(m, null) && hits.length === 0) continue;
    shown++;

    const active = m.id === snap.model_id;
    const card = el("div", "card");
    if (active) card.classList.add("active");
    if (open.has(m.id) || (filter && hits.length)) card.classList.add("open");

    // --- the head: what it is, and one line on what it costs
    const head = el("div", "cardhead");
    const name = el("div", "cardname");
    name.append(el("span", "cardlabel", m.label));
    if (active) name.append(el("span", "badge on", "ACTIVE"));
    else if (m.recommended && !m.installed) name.append(el("span", "badge", "RECOMMENDED"));
    head.append(name);
    head.append(el("div", "cardtrade", m.tradeoff));
    card.append(head);

    // --- the action: one button whose verb is the state it is in
    const act = el("div", "cardact");
    if (m.installing) {
      const p = progress.get(m.id);
      const pct = p?.total ? Math.round((p.received / p.total) * 100) : null;
      const bar = el("div", "bar");
      const fill = el("i");
      if (pct !== null) fill.style.width = `${pct}%`;
      bar.append(fill);
      card.append(bar);

      const phase = (p?.phase ?? "Downloading").toLowerCase();
      act.append(
        el("span", "cardmeta", pct !== null ? `${phase} · ${pct}% of ${m.mb} MB` : phase)
      );
      const stop = el("button", "get", "Cancel");
      stop.onclick = (e) => {
        e.stopPropagation();
        // Friction in proportion to what is about to be thrown away: past
        // halfway, ask; before it, there is little to lose and asking is noise.
        if (pct !== null && pct > 50 && !confirm(`Discard the ${pct}% of ${m.label} already downloaded?`)) {
          return;
        }
        invoke("cancel_install", { modelId: m.id });
      };
      act.append(stop);
    } else if (!m.installed) {
      act.append(
        el("span", "cardmeta", m.speakers.length > 1 ? `${m.speakers.length} voices` : m.accent)
      );
      // The download is the audition. The cost sits on the button, because
      // pressing it is a commitment and a commitment states its price.
      const btn = el("button", "get", `Hear it · ${m.mb} MB`);
      btn.onclick = (e) => {
        e.stopPropagation();
        invoke("audition_or_install", { modelId: m.id, sid: m.speakers[0].sid });
      };
      act.append(btn);
    } else {
      act.append(
        el("span", "cardmeta", m.speakers.length > 1 ? `${m.speakers.length} voices` : m.accent)
      );
      if (m.speakers.length > 1) {
        const btn = el("button", "get", card.classList.contains("open") ? "Hide voices" : "Voices");
        btn.onclick = (e) => {
          e.stopPropagation();
          open.has(m.id) ? open.delete(m.id) : open.add(m.id);
          renderModels();
        };
        act.append(btn);
      } else if (!active) {
        const btn = el("button", "get", "Use");
        btn.onclick = (e) => {
          e.stopPropagation();
          invoke("choose_voice", { modelId: m.id, sid: m.speakers[0].sid });
        };
        act.append(btn);
      }
      const play = el("button", "play", "▶");
      play.title = "Hear this voice";
      play.onclick = (e) => {
        e.stopPropagation();
        invoke("audition", { modelId: m.id, sid: snap.speaker && active ? snap.speaker : m.speakers[0].sid });
      };
      act.append(play);
    }
    card.append(act);

    // --- the speakers, for models that have more than one
    if (m.installed && m.speakers.length > 1) {
      const list = el("div", "voices");
      for (const sp of hits) {
        const row = el("div", "voice");
        if (active && sp.sid === snap.speaker) row.classList.add("on");
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
      card.append(list);
    }

    box.append(card);
  }

  if (!shown) box.append(el("div", "empty", "Nothing matches that."));
}

// ---- events ----

$("search").oninput = (e) => {
  filter = e.target.value.trim().toLowerCase();
  renderModels();
};

// ---- follow-along -------------------------------------------------------
//
// While speaking, the textarea is swapped for a read-only view of the same
// text split into the chunks the engine actually produced, so the highlight can
// land on exactly what is being heard. The engine sends an index; it never
// sends text, and this never re-splits text -- both would be a second source of
// truth that could disagree with the voice.

let chunks = [];

function showFollow(cs) {
  $("preview").hidden = true;
  $("browse").hidden = true;
  chunks = cs;
  const box = $("follow");
  box.innerHTML = "";
  cs.forEach((c, i) => {
    const el = document.createElement("span");
    el.className = "chunk";
    el.dataset.i = String(i);
    el.textContent = c + " ";
    el.title = "Read from here";
    // Seeking is the same call as playing, with a different index -- the
    // engine keeps the enumeration, so a click cannot re-chunk the document
    // into something the highlight no longer matches.
    el.onclick = () => {
      highlight(i);
      invoke("seek", { to: i });
    };
    box.append(el);
  });
  box.hidden = false;
  $("text").hidden = true;
  $("chapters").hidden = true;
}

function hideFollow() {
  $("follow").hidden = true;
  // A book returns to its chapter list; pasted text returns to the box it was
  // pasted into. Either way the pane shows what it showed before.
  if (book) {
    $("chapters").hidden = false;
  } else {
    $("text").hidden = false;
  }
  chunks = [];
}

// When the reader last scrolled by hand. Auto-scroll stands down for a few
// seconds afterwards: a view that yanks itself back while someone is reading
// ahead is worse than one that never scrolls at all.
let scrolledAt = 0;
const SCROLL_TRUCE = 4000;
$("follow").addEventListener("wheel", () => (scrolledAt = Date.now()), { passive: true });
$("follow").addEventListener("touchmove", () => (scrolledAt = Date.now()), { passive: true });

function highlight(index) {
  const box = $("follow");
  box.querySelectorAll(".chunk").forEach((el) => {
    const i = Number(el.dataset.i);
    el.classList.toggle("now", i === index);
    el.classList.toggle("spoken", i < index);
  });
  if (Date.now() - scrolledAt < SCROLL_TRUCE) return;
  // Scroll only when the highlight would otherwise be off screen. Auto-scroll
  // that fires on every chunk fights a reader who has scrolled deliberately.
  const cur = box.querySelector(".chunk.now");
  if (cur) {
    const r = cur.getBoundingClientRect(), b = box.getBoundingClientRect();
    if (r.top < b.top || r.bottom > b.bottom) {
      cur.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  }
}

// ---- resume -------------------------------------------------------------
//
// A saved position is offered, never acted on. Launching an app must not make
// it start talking: of everything Lector does, being heard is the one thing the
// reader cannot take back.

let saved = null;

async function offerResume() {
  saved = await invoke("reading_state");
  if (!saved) return;
  const pct = saved.total ? Math.round((saved.index / saved.total) * 100) : 0;
  $("resumetext").textContent = `You stopped ${pct}% through what you were reading.`;
  $("text").value = saved.text;
  $("resume").hidden = false;
}

$("resumebtn").onclick = async () => {
  if (!saved) return;
  $("resume").hidden = true;
  const cs = await invoke("read_document", { text: saved.text, from: saved.index });
  if (cs.length) {
    showFollow(cs);
    highlight(saved.index);
  }
};

$("resumeno").onclick = () => {
  $("resume").hidden = true;
  invoke("forget_reading");
  saved = null;
};

$("go").onclick = async () => {
  if (snap?.speaking) {
    await invoke("stop");
    hideFollow();
    return;
  }
  const text = $("text").value.trim();
  if (!text) return;
  // One call: it enumerates, starts reading, and returns the very chunks the
  // engine is speaking. Asking separately would be two enumerations.
  $("resume").hidden = true;
  // Pasted text is not part of a book, and the backend clears the open book on
  // this call; the window has to agree or Back would go nowhere.
  book = null;
  $("back").hidden = true;
  renderBooks();
  const cs = await invoke("read_document", { text, from: 0 });
  if (cs.length) showFollow(cs);
};

$("pause").onclick = async () => {
  await invoke(snap?.paused ? "resume" : "pause");
  const s = await invoke("snapshot");
  snap = s;
  render();
};

// True between one chapter ending and the next one starting, so the
// speaking-state poll does not tear the reading view down in the gap.
let advancing = false;

listen("reading", (e) => {
  const { index, total } = e.payload;
  // One past the last chunk is the engine saying the last sample has been
  // *heard*, not merely generated. Everything that happens at the end of a
  // chapter hangs off this.
  if (total && index >= total) {
    ended();
    return;
  }
  if (!$("follow").hidden) highlight(index);
  if (book) book.unit = index;
});

async function ended() {
  if (!book) {
    hideFollow();
    return;
  }
  const next = book.chapter + 1;
  if (next >= book.chapters.length) {
    // The end of the book. Back to the chapter list rather than silence with
    // a stale highlight sitting on the last line.
    book.unit = 0;
    hideFollow();
    return;
  }
  advancing = true;
  try {
    await readChapter(next, 0);
  } finally {
    advancing = false;
  }
}

// ---- the library ---------------------------------------------------------
//
// Books are imported once and read from disk after that. The window holds no
// copy of a book's text: it asks for a chapter's chunks when it needs them, so
// the chunks it highlights are the ones the engine was handed.

let tab = "voices";

function showTab(which) {
  tab = which;
  document.querySelectorAll(".tab").forEach((b) => b.classList.toggle("on", b.dataset.tab === which));
  $("voicespane").hidden = which !== "voices";
  $("librarypane").hidden = which !== "library";
  $("count").textContent =
    which === "voices" ? `${snap?.voice_count ?? 0} voices` : `${shelf.length} book${shelf.length === 1 ? "" : "s"}`;
  if (which === "library") refreshLibrary();
}

document.querySelectorAll(".tab").forEach((b) => {
  b.onclick = () => showTab(b.dataset.tab);
});

async function refreshLibrary() {
  shelf = await invoke("library");
  if (tab === "library") {
    $("count").textContent = `${shelf.length} book${shelf.length === 1 ? "" : "s"}`;
  }
  renderBooks();
}

function renderBooks() {
  const box = $("books");
  box.innerHTML = "";
  if (!shelf.length) {
    box.append(el("div", "empty", "No books yet. Add an EPUB to get started."));
    return;
  }
  for (const b of shelf) {
    const row = el("div", "bookrow");
    if (book && book.id === b.id) row.classList.add("on");

    const art = el("div", "cover");
    if (b.cover) {
      const img = document.createElement("img");
      const cached = covers.get(b.id);
      if (cached) {
        img.src = cached;
      } else {
        // Fetched lazily and once: covers are tens of kilobytes and the shelf
        // is re-rendered on every change.
        invoke("cover", { id: b.id }).then((url) => {
          if (url) {
            covers.set(b.id, url);
            img.src = url;
          }
        });
      }
      art.append(img);
    } else {
      art.append(el("span", null, b.title.slice(0, 1).toUpperCase()));
    }

    const meta = el("div", "bookmeta");
    meta.append(el("div", "booktitle", b.title));
    meta.append(el("div", "bookby", b.author || `${b.chapters} chapters`));

    const del = el("button", "play", "×");
    del.title = "Remove this book";
    del.onclick = (e) => {
      e.stopPropagation();
      invoke("remove_book", { id: b.id }).catch(() => {});
      if (book && book.id === b.id) closeBook();
    };

    row.append(art, meta, del);
    row.onclick = () => openBook(b.id);
    box.append(row);
  }
}

$("import").onclick = () => invoke("import_book");

listen("importing", (e) => {
  $("import").textContent = `Reading ${e.payload.name ?? "it"}…`;
  $("import").disabled = true;
});

function importDone() {
  $("import").textContent = "Add a book…";
  $("import").disabled = false;
}

$("urlform").onsubmit = (e) => {
  e.preventDefault();
  const url = $("url").value.trim();
  if (!url) return;
  $("url").value = "";
  $("url").placeholder = "fetching…";
  invoke("import_url", { url }).finally(() => {
    $("url").placeholder = "…or paste an article's web address";
  });
};

// ---- free books ----------------------------------------------------------
//
// The second and last thing in Lector that reaches the network, after model
// downloads. It fetches in: a search goes out, nothing about what is being
// read ever does.

let results = [];

$("browsebtn").onclick = () => openBrowse("");

async function openBrowse(query) {
  book = null;
  $("browse").hidden = false;
  $("chapters").hidden = true;
  $("text").hidden = true;
  $("follow").hidden = true;
  $("preview").hidden = true;
  $("back").hidden = true;
  $("resume").hidden = true;
  $("results").innerHTML = "";
  $("browsenote").textContent = query ? "Searching…" : "Loading recent releases…";
  try {
    results = await invoke("browse", { query });
    $("browsenote").textContent = query
      ? `${results.length} result${results.length === 1 ? "" : "s"} from Project Gutenberg`
      : "Recently released by Standard Ebooks — carefully typeset public-domain books.";
    renderResults();
  } catch (e) {
    $("browsenote").textContent = String(e);
  }
}

$("browseform").onsubmit = (e) => {
  e.preventDefault();
  openBrowse($("bq").value.trim());
};

function renderResults() {
  const box = $("results");
  box.innerHTML = "";
  if (!results.length) {
    box.append(el("div", "empty", "Nothing found. Try an author or a title."));
    return;
  }
  for (const r of results) {
    const row = el("div", "result");
    const meta = el("div", "bookmeta");
    meta.append(el("div", "booktitle", r.title));
    const by = [r.author, r.source].filter(Boolean).join(" · ");
    meta.append(el("div", "bookby", by));
    if (r.note) meta.append(el("div", "resultnote", r.note));

    const have = shelf.some((b) => b.title === r.title && (!r.author || b.author === r.author));
    const btn = el(
      "button",
      "get",
      have ? "In your library" : r.bytes ? `Get · ${Math.round(r.bytes / 1024)} KB` : "Get"
    );
    if (have) btn.classList.add("done");
    btn.onclick = () => {
      btn.disabled = true;
      btn.textContent = "0%";
      downloading.set(r.id, btn);
      invoke("get_book", { listing: r });
    };
    row.append(meta, btn);
    box.append(row);
  }
}

// Buttons awaiting a download, keyed by listing id -- the same key the
// progress events carry, so a model download and a book download can share one
// listener without either guessing which is which.
const downloading = new Map();

// ---- the PDF preview -----------------------------------------------------
//
// A PDF is not imported until its extraction has been looked at. Reading order
// is not something a PDF records, so a bad extraction narrates confident
// nonsense -- obvious on screen in a second, and thirty seconds of puzzlement
// by ear.

let pending = null;

listen("pdf_preview", (e) => {
  importDone();
  pending = e.payload;
  const lost = e.payload.failed_pages || 0;
  $("previewpages").textContent = lost
    ? `${e.payload.total_pages} pages — ${lost} could not be read and are missing below.`
    : `${e.payload.total_pages} pages, all readable.`;
  $("previewtext").textContent = e.payload.text;
  $("preview").hidden = false;
  $("chapters").hidden = true;
  $("follow").hidden = true;
  $("text").hidden = true;
  $("back").hidden = true;
});

$("previewadd").onclick = () => {
  if (!pending) return;
  invoke("accept_pdf", { path: pending.path, text: pending.text });
  closePreview();
};
$("previewno").onclick = closePreview;

function closePreview() {
  pending = null;
  $("preview").hidden = true;
  if (book) $("chapters").hidden = false;
  else $("text").hidden = false;
}

async function openBook(id) {
  const v = await invoke("open_book", { id });
  if (!v) return;
  book = v;
  renderBooks();
  showChapters();
}

function closeBook() {
  book = null;
  invoke("close_book");
  $("chapters").hidden = true;
  $("follow").hidden = true;
  $("text").hidden = false;
  $("back").hidden = true;
  renderBooks();
}

function showChapters() {
  const box = $("chapters");
  box.innerHTML = "";

  const head = el("div", "chaphead");
  head.append(el("div", "chaptitle", book.title));
  head.append(el("div", "chapby", book.author || ""));
  if (book.warnings > 5) {
    // Said rather than discovered by ear: a rough extraction sounds like a
    // broken app, and the reader deserves to know which it is.
    head.append(el("div", "chapwarn", `This book's markup was uneven — ${book.warnings} spots were repaired on import, so a few passages may read oddly.`));
  }
  box.append(head);

  book.chapters.forEach((title, i) => {
    const row = el("div", "chapter");
    if (i === book.chapter) row.classList.add("at");
    row.append(el("span", "n", String(i + 1)));
    row.append(el("span", "t", title));
    if (i === book.chapter && book.unit > 0) row.append(el("span", "where", "where you left off"));
    row.onclick = () => readChapter(i, i === book.chapter ? book.unit : 0);
    box.append(row);
  });

  box.hidden = false;
  $("text").hidden = true;
  $("follow").hidden = true;
  $("preview").hidden = true;
  $("browse").hidden = true;
  $("back").hidden = true;
  $("resume").hidden = true;
}

async function readChapter(i, from) {
  const cs = await invoke("read_chapter", { id: book.id, chapter: i, from });
  if (!cs.length) return;
  book.chapter = i;
  book.unit = from;
  showFollow(cs);
  if (from > 0) highlight(from);
  $("back").hidden = false;
}

$("back").onclick = async () => {
  await invoke("stop");
  if (book) {
    // Re-open rather than reuse: the position moved while it was being read,
    // and the list should show where the voice actually got to.
    const v = await invoke("open_book", { id: book.id });
    if (v) book = v;
    showChapters();
  }
};

listen("imported", (e) => {
  importDone();
  refreshLibrary();
  showTab("library");
  // Browsing stays put: downloading one book is usually the first of several,
  // and jumping into the reader would throw away the search that found it.
  if ($("browse").hidden) openBook(e.payload.id);
});
listen("library", refreshLibrary);

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
  const btn = downloading.get(e.payload.id);
  if (btn) {
    if (e.payload.phase === "Done") {
      downloading.delete(e.payload.id);
      // Not back to "Get". A 500 KB book over a fast connection is finished
      // before the percentages are readable, so a button that returns to its
      // starting state is the only thing the reader sees -- and it looks like
      // nothing happened.
      btn.textContent = "In your library";
      btn.classList.add("done");
    } else if (e.payload.total) {
      btn.textContent = `${Math.round((e.payload.received / e.payload.total) * 100)}%`;
    } else if (e.payload.received) {
      // No Content-Length: count up in kilobytes rather than sit at 0%.
      btn.textContent = `${Math.round(e.payload.received / 1024)} KB`;
    }
    return;
  }
  progress.set(e.payload.id, e.payload);
  if (e.payload.phase === "Done") {
    progress.delete(e.payload.id);
    refresh();
  } else {
    renderModels();
  }
});
listen("failed", (e) => {
  importDone();
  progress.delete(e.payload.id);
  refresh();
  // A download that was waiting on this id gets its button back, so a failure
  // is something you can retry rather than a row that is stuck disabled.
  const btn = downloading.get(e.payload.id);
  if (btn) {
    downloading.delete(e.payload.id);
    btn.disabled = false;
    btn.textContent = "Retry";
  }
  $("warntext").textContent = e.payload.error;
  $("warnbtn").style.display = "none";
  $("warn").classList.add("show");
});

// Speaking state is polled rather than pushed: it changes far faster than
// anything worth an event, and the window is often not visible.
setInterval(async () => {
  if (document.hidden || advancing) return;
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
refreshLibrary();
offerResume();
