// Canopee Chat frontend. Talks to the embedded Rust runtime via the Tauri
// global API (invoke for commands, listen for push events). No build step.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

let contacts = []; // [{ name, topic }]
let active = null; // selected contact name

async function refreshStatus() {
  try {
    const info = await invoke("get_self");
    $("my-id").textContent = info.contact;
    $("my-id").title = info.contact;
    $("copy-id").disabled = false;
    $("status").textContent =
      info.peers_connected + " peer(s) connected · " +
      (await invoke("get_peers")).length + " in swarm list";
    $("listen").textContent = "listen:\n" + (info.listen_addrs.join("\n") || "—");
    await refreshContacts();
  } catch (e) {
    $("status").textContent = "error: " + e;
  }
}

async function refreshContacts() {
  contacts = await invoke("list_conversations");
  const list = $("peer-list");
  list.innerHTML = "";
  for (const c of contacts) {
    const el = document.createElement("div");
    el.className = "peer" + (active === c.name ? " active" : "");
    el.textContent = c.name;
    el.title = c.topic + "\n" + c.peer_id;
    el.onclick = () => selectContact(c.name);
    list.appendChild(el);
  }
  const composerEnabled = active != null;
  $("text").disabled = !composerEnabled;
  $("send").disabled = !composerEnabled;
}

function selectContact(name) {
  active = name;
  $("empty").style.display = "none";
  refreshContacts();
}

function appendMessage(from, text, dir) {
  $("empty").style.display = "none";
  const box = $("messages");
  const el = document.createElement("div");
  el.className = "msg " + dir;
  if (dir === "in") {
    const sender = document.createElement("div");
    sender.className = "sender";
    sender.textContent = from;
    el.appendChild(sender);
  }
  const body = document.createElement("div");
  body.textContent = text;
  el.appendChild(body);
  box.appendChild(el);
  box.scrollTop = box.scrollHeight;
}

$("copy-id").onclick = async () => {
  const contact = $("my-id").textContent;
  try {
    await navigator.clipboard.writeText(contact);
    $("status").textContent = "ID copied";
  } catch {
    $("status").textContent = "copy blocked — select the string and copy manually";
  }
};

$("add-peer").onclick = async () => {
  const name = $("name").value.trim();
  const contact = $("contact").value.trim();
  const err = $("peer-error");
  err.textContent = "";
  if (!name || !contact) {
    err.textContent = "name and contact string are both required";
    return;
  }
  try {
    await invoke("add_peer", { args: { name, contact } });
    $("contact").value = "";
    await refreshStatus();
  } catch (e) {
    err.textContent = e;
  }
};

$("send").onclick = sendMessage;
$("text").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    sendMessage();
  }
});

async function sendMessage() {
  const text = $("text").value;
  if (!text.trim() || !active) return;
  $("text").value = "";
  appendMessage(active, text, "out");
  try {
    await invoke("send_message", { args: { name: active, text } });
  } catch (e) {
    $("status").textContent = "send failed: " + e;
  }
}

async function main() {
  await listen("chat-message", (event) => {
    const { from, text } = event.payload;
    appendMessage(from, text, "in");
  });
  // Dialog the manual (non-mDNS) dial path behind a prompt for LAN testing.
  // Most same-LAN setups never need it — mDNS finds peers automatically.
  window.dial = async (addr) => {
    try {
      await invoke("dial", { addr });
      $("status").textContent = "dialed " + addr;
    } catch (e) {
      $("status").textContent = "dial failed: " + e;
    }
  };
  await refreshStatus();
  setInterval(refreshStatus, 5000);
}

main().catch((e) => {
  $("status").textContent = "startup error: " + e;
});