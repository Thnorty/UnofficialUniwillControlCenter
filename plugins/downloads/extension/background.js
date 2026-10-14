// Reports the downloads in progress to Unofficial Uniwill Control Center's Downloads plugin while there are any.
const ADDRESS = "http://127.0.0.1:47823/plugins/downloads";
let timer = null;

async function post(path, body) {
  try {
    await fetch(ADDRESS + path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  } catch (e) {
    // The app isn't running.
  }
}

async function report() {
  const items = await chrome.downloads.search({ state: "in_progress" });
  const downloads = items.map((d) => ({
    id: d.id,
    name: (d.filename || d.url || "").split(/[\\/]/).pop(),
    received: d.bytesReceived,
    total: d.totalBytes,
    state: d.state,
    paused: d.paused,
  }));
  await post("/progress", { downloads });
  if (downloads.length === 0 && timer) {
    clearInterval(timer);
    timer = null;
  }
}

function start() {
  if (!timer) {
    timer = setInterval(report, 500);
  }
  report();
}

chrome.downloads.onCreated.addListener(start);
chrome.downloads.onChanged.addListener((delta) => {
  if (delta.state && delta.state.current === "complete") {
    post("/done", { id: delta.id });
  }
  start();
});
chrome.runtime.onStartup.addListener(start);
