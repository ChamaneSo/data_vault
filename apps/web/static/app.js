"use strict";
const $ = id => document.getElementById(id);
const apiRoot = "/web-api";
const state = {
  account: null, buckets: [], bucket: null, bucketCursor: null,
  folder: "", page: 0, cursors: [null], next: null,
};
let bucketRequest = 0;
let objectRequest = 0;
let searchTimer;
let noticeTimer;

function element(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
}

function message(id, text = "") {
  $(id).textContent = text;
  $(id).hidden = !text;
}

function toast(text) {
  clearTimeout(noticeTimer);
  message("notice", text);
  noticeTimer = setTimeout(() => message("notice"), 3800);
}

async function api(path, options = {}) {
  const response = await fetch(apiRoot + path, {
    ...options, credentials: "same-origin", cache: "no-store",
    headers: { "X-DataVault-Request": "1", ...options.headers },
  });
  if (!response.ok) {
    const error = await response.json().catch(() => ({}));
    if (response.status === 401 && state.account) {
      showLogin();
      message("connect-error", "Your session ended. Connect again to continue.");
    }
    throw new Error(error.error || `Request failed (HTTP ${response.status}).`);
  }
  return response.status === 204 || options.method === "HEAD" ? null : response.json();
}

function jsonPost(value) {
  return { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(value) };
}

async function checkHealth() {
  let ready = false;
  try { ready = (await api("/health")).ready; } catch { /* Connection may be unavailable. */ }
  $("health").dataset.state = ready ? "online" : "offline";
  $("health").replaceChildren(element("i"), document.createTextNode(ready ? "Server online" : "Server unavailable"));
}

function showLogin() {
  bucketRequest++;
  objectRequest++;
  clearTimeout(searchTimer);
  state.account = null;
  state.buckets = [];
  state.bucket = null;
  $("objects").replaceChildren();
  $("bucket-list").replaceChildren();
  $("account-label").textContent = "";
  $("workspace").hidden = true;
  $("disconnect").hidden = true;
  $("login").hidden = false;
  $("secret-key").value = "";
  $("secret-key").type = "password";
  $("reveal").textContent = "Show";
  $("reveal").setAttribute("aria-pressed", "false");
  for (const dialog of document.querySelectorAll("dialog[open]")) dialog.close();
}

async function showWorkspace(session) {
  state.account = session.account;
  $("account-label").textContent = session.account;
  $("login").hidden = true;
  $("workspace").hidden = false;
  $("disconnect").hidden = false;
  await loadBuckets();
}

$("reveal").addEventListener("click", () => {
  const visible = $("secret-key").type === "password";
  $("secret-key").type = visible ? "text" : "password";
  $("reveal").textContent = visible ? "Hide" : "Show";
  $("reveal").setAttribute("aria-pressed", String(visible));
});

$("connect-form").addEventListener("submit", async event => {
  event.preventDefault();
  const button = $("connect-button");
  button.disabled = true;
  button.textContent = "Connecting…";
  message("connect-error");
  try {
    const session = await api("/session", jsonPost({
      access_key: $("access-key").value.trim(), secret_key: $("secret-key").value,
    }));
    $("connect-form").reset();
    await showWorkspace(session);
  } catch (error) {
    message("connect-error", error.message);
  } finally {
    button.disabled = false;
    button.textContent = "Connect to DataVault";
  }
});

$("disconnect").addEventListener("click", async () => {
  try {
    await api("/session", { method: "DELETE" });
    showLogin();
    message("connect-error");
    $("access-key").focus();
  } catch (error) { message("workspace-error", error.message); }
});

function renderBuckets() {
  const list = $("bucket-list");
  list.replaceChildren();
  for (const name of state.buckets) {
    const button = element("button", undefined, "bucket-item");
    const icon = element("span", undefined, "bucket-icon");
    icon.setAttribute("aria-hidden", "true");
    button.append(icon, element("span", name));
    button.setAttribute("aria-current", String(state.bucket === name));
    button.addEventListener("click", () => selectBucket(name));
    list.append(button);
  }
  $("more-buckets").hidden = !state.bucketCursor;
}

async function loadBuckets(append = false, preferred = null) {
  const version = ++bucketRequest;
  $("more-buckets").disabled = true;
  message("bucket-message", "Loading buckets…");
  try {
    const query = new URLSearchParams();
    if (append && state.bucketCursor) query.set("cursor", state.bucketCursor);
    const result = await api("/buckets?" + query);
    if (version !== bucketRequest) return;
    state.buckets = [...new Set([...(append ? state.buckets : []), ...result.buckets])];
    if (preferred && !state.buckets.includes(preferred)) {
      state.buckets.unshift(preferred);
    }
    state.bucketCursor = result.next || null;
    renderBuckets();
    message("bucket-message", state.buckets.length ? "" : "No accessible buckets.");
    if (!append) {
      const selected = preferred || (state.buckets.includes(state.bucket) ? state.bucket : state.buckets[0]);
      if (selected && selected === state.bucket) await loadObjects(0);
      else await selectBucket(selected || null);
    }
  } catch (error) {
    if (version === bucketRequest) message("bucket-message", error.message);
  } finally {
    if (version === bucketRequest) $("more-buckets").disabled = false;
  }
}

function setEmpty(title, detail) {
  $("empty-title").textContent = title;
  $("empty-detail").textContent = detail;
  $("empty").hidden = !title;
}

async function selectBucket(name) {
  clearTimeout(searchTimer);
  state.bucket = name;
  state.folder = "";
  $("search").value = "";
  $("bucket-title").textContent = name || "Your buckets";
  $("upload-open").disabled = !name;
  $("search").disabled = !name;
  renderBuckets();
  await loadObjects(0);
}

function breadcrumbs() {
  const nav = $("breadcrumbs");
  nav.replaceChildren();
  if (!state.bucket) return;
  const chunks = state.folder ? state.folder.slice(0, -1).split("/") : [];
  let prefix = "";
  function crumb(label, value, current) {
    if (nav.childNodes.length) nav.append(document.createTextNode(" / "));
    const node = element(current ? "span" : "button", label);
    if (current) node.setAttribute("aria-current", "location");
    else node.addEventListener("click", () => openFolder(value));
    nav.append(node);
  }
  crumb("All objects", "", !chunks.length);
  chunks.forEach((part, i) => {
    prefix += part + "/";
    crumb(part || "/", prefix, i === chunks.length - 1);
  });
}

function openFolder(prefix) {
  clearTimeout(searchTimer);
  state.folder = prefix;
  $("search").value = "";
  loadObjects(0);
}

function formatSize(size) {
  if (size < 1024) return size + " B";
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let n = size, i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < units.length - 1);
  return new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(n) + " " + units[i];
}

function bucketPath() {
  return "/buckets/" + encodeURIComponent(state.bucket);
}

function addRow(key, folder, size, modified) {
  const row = element("tr");
  const name = element("td");
  const wrap = element("div", undefined, "name-cell");
  const icon = element("span", undefined, folder ? "folder-icon" : "file-icon");
  icon.setAttribute("aria-hidden", "true");
  const label = key.slice(state.folder.length) || key;
  const title = element(folder ? "button" : "span", label, folder ? "folder-link" : undefined);
  title.title = key;
  if (folder) title.addEventListener("click", () => openFolder(key));
  wrap.append(icon, title);
  name.append(wrap);
  const date = element("td", modified ? new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(new Date(modified)) : "—");
  if (modified) date.title = new Date(modified).toLocaleString();
  const action = element("td");
  if (!folder) {
    const link = element("a", "Download", "download-link");
    const path = bucketPath() + "/download?" + new URLSearchParams({ key });
    link.href = apiRoot + path;
    link.download = key.split("/").pop() || "object";
    link.setAttribute("aria-label", "Download " + key);
    link.addEventListener("click", async event => {
      if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      try {
        await api(path, { method: "HEAD" });
        const download = element("a");
        download.href = link.href;
        download.download = link.download;
        document.body.append(download);
        download.click();
        download.remove();
      } catch (error) { message("workspace-error", error.message); }
    });
    action.append(link);
  }
  row.append(name, element("td", folder ? "—" : formatSize(size)), date, action);
  $("objects").append(row);
}

async function loadObjects(page) {
  const version = ++objectRequest;
  if (page === 0) state.cursors = [null];
  breadcrumbs();
  $("objects").replaceChildren();
  message("workspace-error");
  $("page-summary").textContent = "";
  $("previous").disabled = true;
  $("next").disabled = true;
  if (!state.bucket) {
    setEmpty("No bucket selected", "Create a bucket or ask for access to an existing one.");
    return;
  }
  setEmpty("Loading objects…", "");
  const search = $("search").value;
  const query = new URLSearchParams({ prefix: state.folder + search });
  if (search) query.set("flat", "1");
  if (state.cursors[page]) query.set("cursor", state.cursors[page]);
  try {
    const result = await api(bucketPath() + "/objects?" + query);
    if (version !== objectRequest) return;
    state.page = page;
    state.next = result.next || null;
    for (const folder of result.folders) addRow(folder, true);
    for (const item of result.objects) addRow(item.key, false, item.size, item.modified);
    const count = result.folders.length + result.objects.length;
    setEmpty(count ? "" : search ? "No matching objects" : "No objects here yet",
      search ? "Try a different key prefix." : "Upload a file to this bucket or folder.");
    $("page-summary").textContent = `${count} ${count === 1 ? "item" : "items"} · Page ${page + 1}`;
    $("previous").disabled = page === 0;
    $("next").disabled = !state.next;
  } catch (error) {
    if (version !== objectRequest) return;
    setEmpty("Could not load objects", "Use Refresh to try again.");
    message("workspace-error", error.message);
  }
}

$("more-buckets").addEventListener("click", () => loadBuckets(true));
$("refresh").addEventListener("click", () => { checkHealth(); loadBuckets(); });
$("next").addEventListener("click", () => {
  if (!state.next) return;
  state.cursors[state.page + 1] = state.next;
  loadObjects(state.page + 1);
});
$("previous").addEventListener("click", () => loadObjects(Math.max(0, state.page - 1)));
$("search").addEventListener("input", () => {
  clearTimeout(searchTimer);
  objectRequest++;
  $("previous").disabled = true;
  $("next").disabled = true;
  searchTimer = setTimeout(() => loadObjects(0), 220);
});

function busy(dialog, value, caption) {
  dialog.dataset.busy = String(value);
  for (const input of dialog.querySelectorAll("button, input")) input.disabled = value;
  dialog.querySelector('[type="submit"]').textContent = caption;
}
for (const dialog of document.querySelectorAll("dialog")) {
  dialog.addEventListener("cancel", event => { if (dialog.dataset.busy === "true") event.preventDefault(); });
  for (const button of dialog.querySelectorAll(".close")) button.addEventListener("click", () => dialog.close());
}

$("new-bucket").addEventListener("click", () => {
  $("bucket-form").reset(); message("bucket-error"); $("bucket-dialog").showModal();
  $("bucket-name").focus();
});
$("bucket-form").addEventListener("submit", async event => {
  event.preventDefault();
  const dialog = $("bucket-dialog");
  const name = $("bucket-name").value.trim();
  message("bucket-error");
  busy(dialog, true, "Creating…");
  try {
    await api("/buckets", jsonPost({ name }));
    dialog.close();
    await loadBuckets(false, name);
    toast("Bucket created.");
  } catch (error) { message("bucket-error", error.message); }
  finally { busy(dialog, false, "Create bucket"); }
});

$("upload-open").addEventListener("click", () => {
  $("upload-form").reset();
  $("object-key").value = state.folder;
  message("upload-error"); $("upload-dialog").showModal();
});
$("file").addEventListener("change", () => {
  const file = $("file").files[0];
  if (file) $("object-key").value = state.folder + file.name;
});
$("upload-form").addEventListener("submit", async event => {
  event.preventDefault();
  const file = $("file").files[0];
  const key = $("object-key").value;
  if (!file) return;
  if (file.size > 128 * 1024 * 1024) { message("upload-error", "Choose a file of 128 MiB or smaller."); return; }
  const dialog = $("upload-dialog");
  const query = new URLSearchParams({ key, replace: $("replace").checked ? "1" : "0" });
  message("upload-error");
  busy(dialog, true, "Uploading…");
  try {
    await api(bucketPath() + "/object?" + query, {
      method: "PUT", headers: { "Content-Type": file.type || "application/octet-stream" }, body: file,
    });
    dialog.close();
    $("search").value = "";
    state.folder = key.slice(0, key.lastIndexOf("/") + 1);
    await loadObjects(0);
    toast("File uploaded.");
  } catch (error) { message("upload-error", error.message); }
  finally { busy(dialog, false, "Upload"); }
});

async function start() {
  checkHealth();
  setInterval(() => { if (!document.hidden) checkHealth(); }, 30000);
  try {
    const session = await api("/session");
    if (session.account) await showWorkspace(session);
    else showLogin();
  } catch (error) { message("connect-error", error.message); }
}
start();
