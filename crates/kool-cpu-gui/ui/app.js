const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);

$("win-min").addEventListener("click", () => appWindow.minimize());
$("win-close").addEventListener("click", () => appWindow.close());

let currentPage = "monitor";
let lastSnapshot = null;
let sliderDirty = false;
let chartPoints = [];

function tempColorVar(c) {
  if (c >= 80) return "var(--error)";
  if (c >= 70) return "var(--warm)";
  return "var(--cool)";
}

function updateSliderFill() {
  const el = $("cap-slider");
  const pct = ((el.value - el.min) / (el.max - el.min)) * 100;
  el.style.background = `linear-gradient(to right, var(--primary) 0%, var(--primary) ${pct}%, var(--surface-high) ${pct}%, var(--surface-high) 100%)`;
}

function setSliderValue(mhz, dirty) {
  const el = $("cap-slider");
  el.value = String(mhz);
  sliderDirty = dirty;
  $("slider-label").textContent = `${(mhz / 1000).toFixed(2)} GHz (${mhz} MHz)`;
  $("slider-label").classList.toggle("dirty", dirty);
  $("apply-btn").disabled = !dirty;
  updateSliderFill();
}

function renderChart(history, autoOn, autoCfg) {
  const W = 400, H = 116;
  const padL = 4, padR = 32, padT = 10, padB = 10;
  const plotW = W - padL - padR;
  const minT = 30, maxT = 100;
  const yFor = (t) => H - padB - (Math.max(0, Math.min(1, (t - minT) / (maxT - minT)))) * (H - padT - padB);

  const grid = $("chart-grid");
  if (!grid.hasChildNodes()) {
    let html = "";
    for (const t of [40, 60, 80, 100]) {
      const y = yFor(t).toFixed(1);
      html += `<line class="chart-grid-line" x1="${padL}" y1="${y}" x2="${W - padR}" y2="${y}"></line>`;
      html += `<text class="chart-grid-label" x="${W - 2}" y="${(Number(y) + 3).toFixed(1)}" text-anchor="end">${t}°</text>`;
    }
    grid.innerHTML = html;
  }

  const guides = $("chart-guides");
  if (autoOn && autoCfg) {
    const yHi = yFor(autoCfg.high_c).toFixed(1);
    const yLo = yFor(autoCfg.low_c).toFixed(1);
    guides.innerHTML =
      `<line class="chart-guide-hi" x1="${padL}" y1="${yHi}" x2="${W - padR}" y2="${yHi}"></line>` +
      `<line class="chart-guide-lo" x1="${padL}" y1="${yLo}" x2="${W - padR}" y2="${yLo}"></line>`;
  } else {
    guides.innerHTML = "";
  }

  const empty = $("chart-empty");
  if (!history || history.length < 2) {
    empty.classList.remove("hidden");
    $("chart-line").setAttribute("d", "");
    $("chart-area").setAttribute("d", "");
    $("chart-dot").classList.add("hidden");
    $("chart-dot-glow").classList.add("hidden");
    chartPoints = [];
    return;
  }
  empty.classList.add("hidden");

  const maxSamples = 60;
  const stepX = plotW / (maxSamples - 1);
  const startX = W - padR - (history.length - 1) * stepX;
  chartPoints = history.map((t, i) => ({ x: startX + i * stepX, y: yFor(t), t }));

  const lineD = chartPoints.map((p, i) => `${i === 0 ? "M" : "L"}${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" ");
  const baseY = H - padB;
  const first = chartPoints[0];
  const last = chartPoints[chartPoints.length - 1];
  const areaD = `${lineD} L${last.x.toFixed(1)},${baseY} L${first.x.toFixed(1)},${baseY} Z`;

  const col = tempColorVar(last.t);
  $("chart-line").setAttribute("d", lineD);
  $("chart-line").setAttribute("stroke", col);
  $("chart-area").setAttribute("d", areaD);
  $("grad-stop-top").setAttribute("stop-color", col);
  $("grad-stop-bot").setAttribute("stop-color", col);

  for (const id of ["chart-dot", "chart-dot-glow"]) {
    const dot = $(id);
    dot.setAttribute("cx", last.x.toFixed(1));
    dot.setAttribute("cy", last.y.toFixed(1));
    dot.setAttribute("fill", col);
    dot.classList.remove("hidden");
  }
}

function renderSnapshot(s) {
  lastSnapshot = s;
  document.documentElement.setAttribute("data-theme", s.settings.light_theme ? "light" : "dark");

  $("cpu-name").textContent = s.cpu_name || "Detecting processor...";

  // Status chip
  const chip = $("status-chip");
  chip.className = "chip";
  if (s.is_error) {
    chip.classList.add("chip-error");
    chip.textContent = "Attention";
  } else if (s.applying) {
    chip.classList.add("chip-primary");
    chip.textContent = "Applying";
  } else if (s.auto_enabled) {
    chip.classList.add("chip-primary");
    chip.textContent = "Auto";
  } else if (s.cap_mhz != null) {
    chip.classList.add("chip-primary");
    chip.textContent = "Capped";
  } else {
    chip.classList.add("chip-neutral");
    chip.textContent = "Full speed";
  }

  // Telemetry readout
  const tempEl = $("temp-readout");
  if (s.temp_c != null) {
    tempEl.textContent = `${s.temp_c.toFixed(1)}°C`;
    tempEl.style.color = tempColorVar(s.temp_c);
  } else {
    tempEl.textContent = "--.-°C";
    tempEl.style.color = "var(--on-variant)";
  }

  const capStr = s.cap_mhz != null ? `${(s.cap_mhz / 1000).toFixed(2)} GHz cap` : "Unlimited";
  const pwrSuffix = s.power_source === "Ac" ? " · AC" : s.power_source === "Battery" ? " · Battery" : "";
  $("cap-readout").textContent = `${capStr}${pwrSuffix}`;

  if (s.history && s.history.length > 0) {
    const min = Math.min(...s.history);
    const max = Math.max(...s.history);
    const avg = s.history.reduce((a, b) => a + b, 0) / s.history.length;
    $("stats-readout").textContent = `${Math.round(min)}° / ${Math.round(avg)}° / ${Math.round(max)}°`;
  } else {
    $("stats-readout").textContent = "--° / --° / --°";
  }

  renderChart(s.history, s.auto_enabled, s.auto_cfg);

  // Segmented profile buttons
  const capKey = s.cap_mhz == null ? "none" : String(s.cap_mhz);
  document.querySelectorAll(".seg-btn").forEach((btn) => {
    btn.classList.toggle("active", !s.auto_enabled && btn.dataset.cap === capKey);
    btn.disabled = s.applying;
  });

  // Manual slider sync (unless user is mid-edit)
  if (!sliderDirty && s.cap_mhz != null) {
    setSliderValue(s.cap_mhz, false);
  } else if (!sliderDirty) {
    updateSliderFill();
  }
  $("manual-row").classList.toggle("disabled", s.auto_enabled || s.applying);

  // Auto Thermal Mode
  $("auto-toggle").checked = s.auto_enabled;
  $("auto-panel").classList.toggle("hidden", !s.auto_enabled);
  if (document.activeElement?.closest("#auto-panel") == null) {
    $("auto-high").value = s.auto_cfg.high_c;
    $("auto-low").value = s.auto_cfg.low_c;
    $("auto-min").value = s.auto_cfg.min_mhz;
    $("auto-max").value = s.auto_cfg.max_mhz;
    $("auto-step").value = s.auto_cfg.step_mhz;
  }

  // Settings toggles
  $("set-light").checked = s.settings.light_theme;
  $("set-tray").checked = s.settings.show_in_tray;
  $("set-startup").checked = s.settings.run_on_startup;
  $("set-acdc").checked = s.settings.ac_dc_enabled;
  $("set-reset").checked = s.settings.reset_on_exit;
  $("acdc-panel").classList.toggle("hidden", !s.settings.ac_dc_enabled);

  const acKey = s.settings.ac_cap_mhz == null ? "none" : String(s.settings.ac_cap_mhz);
  const dcKey = s.settings.dc_cap_mhz == null ? "none" : String(s.settings.dc_cap_mhz);
  document.querySelectorAll('.mini-pills[data-target="ac"] .mini-pill').forEach((b) => {
    b.classList.toggle("active", b.dataset.cap === acKey);
  });
  document.querySelectorAll('.mini-pills[data-target="dc"] .mini-pill').forEach((b) => {
    b.classList.toggle("active", b.dataset.cap === dcKey);
  });

  // Updates
  $("app-version").textContent = `v${s.version}`;
  $("config-path").textContent = `Saved to ${s.config_path}`;
  const upd = s.update;
  const dot = $("update-dot");
  const uBtn = $("update-btn");
  const uDesc = $("update-desc");
  dot.classList.toggle("hidden", !(upd.kind === "available" && currentPage === "monitor"));
  uDesc.style.color = "";
  if (upd.kind === "available") {
    uDesc.textContent = `${upd.tag} is available`;
    uDesc.style.color = "var(--primary)";
    uBtn.textContent = "Update";
    uBtn.className = "pill-btn";
    uBtn.disabled = false;
  } else if (upd.kind === "checking" || upd.kind === "updating") {
    uDesc.textContent = upd.kind === "checking" ? "Checking for updates..." : "Downloading & verifying SHA-256...";
    uBtn.textContent = "Working...";
    uBtn.className = "pill-btn pill-tonal";
    uBtn.disabled = true;
  } else if (upd.kind === "up_to_date") {
    uDesc.textContent = "You are on the latest version";
    uDesc.style.color = "var(--cool)";
    uBtn.textContent = "Check";
    uBtn.className = "pill-btn pill-tonal";
    uBtn.disabled = false;
  } else if (upd.kind === "error") {
    uDesc.textContent = upd.message || "Update check failed";
    uDesc.style.color = "var(--error)";
    uBtn.textContent = "Check";
    uBtn.className = "pill-btn pill-tonal";
    uBtn.disabled = false;
  } else {
    uDesc.textContent = "Check GitHub Releases for new versions";
    uBtn.textContent = "Check";
    uBtn.className = "pill-btn pill-tonal";
    uBtn.disabled = false;
  }

  // Footer
  $("status-text").textContent = s.status;
  $("footer").classList.toggle("error", s.is_error);
}

// Navigation
$("nav-btn").addEventListener("click", () => {
  currentPage = currentPage === "monitor" ? "settings" : "monitor";
  const inSettings = currentPage === "settings";
  $("view-monitor").classList.toggle("hidden", inSettings);
  $("view-settings").classList.toggle("hidden", !inSettings);
  $("icon-settings").classList.toggle("hidden", inSettings);
  $("icon-back").classList.toggle("hidden", !inSettings);
  $("nav-btn").title = inSettings ? "Back to monitor" : "Settings";
  if (lastSnapshot) renderSnapshot(lastSnapshot);
});

// Profile buttons
document.querySelectorAll(".seg-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    const cap = btn.dataset.cap === "none" ? null : Number(btn.dataset.cap);
    sliderDirty = false;
    if (cap != null) setSliderValue(cap, false);
    invoke("set_cap", { mhz: cap, disableAuto: true });
  });
});

// Manual slider
$("cap-slider").addEventListener("input", (e) => {
  const mhz = Number(e.target.value);
  const dirty = !lastSnapshot || lastSnapshot.cap_mhz !== mhz;
  setSliderValue(mhz, dirty);
});
$("apply-btn").addEventListener("click", () => {
  const mhz = Number($("cap-slider").value);
  sliderDirty = false;
  $("apply-btn").disabled = true;
  $("slider-label").classList.remove("dirty");
  invoke("set_cap", { mhz, disableAuto: false });
});

// Auto thermal mode
function sendAutoConfig() {
  const highC = Number($("auto-high").value);
  const lowC = Number($("auto-low").value);
  const minMhz = Number($("auto-min").value);
  const maxMhz = Number($("auto-max").value);
  const stepMhz = Number($("auto-step").value);
  const errEl = $("auto-error");
  if (lowC >= highC) {
    errEl.textContent = "Relax temp must be lower than Throttle temp";
    errEl.classList.remove("hidden");
    return;
  }
  if (minMhz < 400 || minMhz >= maxMhz || stepMhz <= 0) {
    errEl.textContent = "Invalid frequency range or step";
    errEl.classList.remove("hidden");
    return;
  }
  errEl.classList.add("hidden");
  invoke("set_auto", {
    enabled: $("auto-toggle").checked,
    highC,
    lowC,
    minMhz,
    maxMhz,
    stepMhz,
  });
}
$("auto-toggle").addEventListener("change", sendAutoConfig);
for (const id of ["auto-high", "auto-low", "auto-min", "auto-max", "auto-step"]) {
  $(id).addEventListener("change", sendAutoConfig);
}

// Settings toggles
$("set-light").addEventListener("change", (e) => invoke("update_setting", { key: "light_theme", value: e.target.checked }));
$("set-tray").addEventListener("change", (e) => invoke("update_setting", { key: "show_in_tray", value: e.target.checked }));
$("set-startup").addEventListener("change", (e) => invoke("update_setting", { key: "run_on_startup", value: e.target.checked }));
$("set-acdc").addEventListener("change", (e) => invoke("update_setting", { key: "ac_dc_enabled", value: e.target.checked }));
$("set-reset").addEventListener("change", (e) => invoke("update_setting", { key: "reset_on_exit", value: e.target.checked }));

document.querySelectorAll(".mini-pills").forEach((group) => {
  const target = group.dataset.target;
  group.querySelectorAll(".mini-pill").forEach((btn) => {
    btn.addEventListener("click", () => {
      const cap = btn.dataset.cap === "none" ? null : Number(btn.dataset.cap);
      invoke("set_power_profile", { target, mhz: cap });
    });
  });
});

// Updates button
$("update-btn").addEventListener("click", () => {
  if (lastSnapshot?.update?.kind === "available") {
    invoke("install_update");
  } else {
    invoke("check_updates");
  }
});

// Chart hover tooltip
const svg = $("temp-chart");
svg.addEventListener("mousemove", (e) => {
  if (chartPoints.length < 2) return;
  const rect = svg.getBoundingClientRect();
  const relX = ((e.clientX - rect.left) / rect.width) * 400;
  let best = null;
  let bestDist = Infinity;
  for (const p of chartPoints) {
    const d = Math.abs(p.x - relX);
    if (d < bestDist) { bestDist = d; best = p; }
  }
  if (best && bestDist < 18) {
    const hl = $("chart-hover-line");
    const hd = $("chart-hover-dot");
    const tip = $("chart-tooltip");
    hl.setAttribute("x1", best.x.toFixed(1));
    hl.setAttribute("x2", best.x.toFixed(1));
    hl.classList.remove("hidden");
    hd.setAttribute("cx", best.x.toFixed(1));
    hd.setAttribute("cy", best.y.toFixed(1));
    hd.classList.remove("hidden");
    tip.textContent = `${best.t.toFixed(1)}°C`;
    tip.style.left = `${(best.x / 400) * 100}%`;
    tip.style.top = `${(best.y / 116) * 100}%`;
    tip.classList.remove("hidden");
  }
});
svg.addEventListener("mouseleave", () => {
  $("chart-hover-line").classList.add("hidden");
  $("chart-hover-dot").classList.add("hidden");
  $("chart-tooltip").classList.add("hidden");
});

// Bootstrap
updateSliderFill();
listen("state-changed", (ev) => renderSnapshot(ev.payload));
invoke("get_state").then(renderSnapshot);
