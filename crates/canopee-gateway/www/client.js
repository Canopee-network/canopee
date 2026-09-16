/* CanopeeWeb: a tiny promise-based browser client for the canopee-gateway
 * WebSocket bridge. Speaks the JSON protocol from
 * crates/canopee-gateway/src/protocol.rs — no build step, plain ES2017.
 *
 * Usage:
 *   const c = new CanopeeWeb("ws://127.0.0.1:PORT/?token=…");
 *   await c.connect();
 *   const identity = await c.identity();
 *   const id = await c.put("hello canopee");
 *   const obj = await c.get(id);
 *   const sub = c.subscribe("chat", msg => console.log(msg.text));
 *   await c.publish("chat", "hi from the browser");
 */
(function (global) {
  "use strict";

  class CanopeeWeb {
    constructor(url) {
      this.url = url;
      this._ws = null;
      this._pending = []; // FIFO of {resolve, reject} for ok/error responses
      this._listeners = []; // functions called with every pubSub message
      this._onerror = null;
    }

    /** Opens the WebSocket and resolves once the gateway replies. Rejects if
     * the gateway is unreachable or the token is refused. */
    connect() {
      return new Promise((resolve, reject) => {
        const ws = new WebSocket(this.url);
        this._ws = ws;
        ws.onopen = () => {
          this._attach();
          resolve(this);
        };
        ws.onerror = () => {
          this._drop();
          reject(new Error("canopee gateway unavailable at " + this.url));
        };
        ws.onclose = () => {
          this._drop();
        };
      });
    }

    close() {
      if (this._ws) this._ws.close();
    }

    /** Registers a handler called with every pub/sub message
     * ({topic, source, text, dataB64}). Dropping the connection closes the
     * subscription, matching the SDK. */
    subscribe(topic, onMessage) {
      this._listeners.push(onMessage);
      this._send({ op: "subscribe", topic });
      return { close: () => this._ws.close() };
    }

    /** Fires for gateway errors that can't be tied to a request. */
    onError(fn) {
      this._onerror = fn;
    }

    // ---- identity ----
    identity() {
      return this._request("identity").then((r) => r.identity);
    }

    // ---- storage ----
    put(text) {
      return this._request("put", { text }).then((r) => r.id);
    }
    get(id) {
      return this._request("get", { id }).then((r) => r.object);
    }
    list() {
      return this._request("list").then((r) => r.objects);
    }
    export(id) {
      return this._request("export", { id }).then((r) => r.object);
    }
    import(object) {
      return this._request("import", { bundle: { version: 1, object } });
    }

    // ---- network ----
    peers() {
      return this._request("peers").then((r) => r.peers);
    }
    dial(addr) {
      return this._request("dial", { addr });
    }
    announce(id) {
      return this._request("announce", { id });
    }
    findProviders(id) {
      return this._request("findProviders", { id }).then((r) => r.peerIds);
    }
    fetchObject(peerId, id) {
      return this._request("fetchObject", { peerId, id }).then((r) => r.object);
    }

    // ---- app pointers ----
    publishAppPointer(name, manifest) {
      return this._request("publishAppPointer", { name, manifest });
    }
    resolveAppPointer(owner, name) {
      return this._request("resolveAppPointer", { owner, name }).then(
        (r) => r.manifest
      );
    }
    publishPointer(name, target) {
      return this._request("publishPointer", { name, target });
    }
    resolvePointer(owner, name) {
      return this._request("resolvePointer", { owner, name }).then(
        (r) => r.pointer
      );
    }

    // ---- user records ----
    loadProfile() {
      return this._request("loadProfile").then((r) => r.profile);
    }
    saveProfile(profile) {
      return this._request("saveProfile", { profile }).then((r) => r.id);
    }
    loadContactList() {
      return this._request("loadContactList").then((r) => r.list);
    }
    saveContactList(list) {
      return this._request("saveContactList", { list }).then((r) => r.id);
    }
    loadHomeIndex() {
      return this._request("loadHomeIndex").then((r) => r.index);
    }
    saveHomeIndex(index) {
      return this._request("saveHomeIndex", { index }).then((r) => r.id);
    }
    setHomeEntryShared(name, shared) {
      return this._request("setHomeEntryShared", { name, shared }).then(
        (r) => r.id
      );
    }
    shareObject(name, object, app) {
      return this._request("shareObject", { name, object, app }).then(
        (r) => r.id
      );
    }

    // ---- usernames ----
    claimUsername(username) {
      return this._request("claimUsername", { username });
    }
    showUsername() {
      return this._request("showUsername").then((r) => r.username);
    }
    resolveUsername(username) {
      return this._request("resolveUsername", { username }).then(
        (r) => r.owner
      );
    }

    // ---- pub/sub ----
    publish(topic, text) {
      return this._request("publish", { topic, text });
    }

    // ---- internals ----

    _send(body) {
      this._ws.send(JSON.stringify(body));
    }

    _request(op, params) {
      const body = Object.assign({ op }, params || {});
      this._send(body);
      return new Promise((resolve, reject) => {
        this._pending.push({ resolve, reject });
      });
    }

    _attach() {
      const ws = this._ws;
      ws.onmessage = (ev) => {
        let msg;
        try {
          msg = JSON.parse(ev.data);
        } catch (_) {
          return;
        }
        switch (msg.op) {
          case "ok": {
            const pending = this._pending.shift();
            if (pending) pending.resolve(msg.result);
            break;
          }
          case "error": {
            const pending = this._pending.shift();
            if (pending) pending.reject(new Error(msg.message));
            else if (this._onerror) this._onerror(new Error(msg.message));
            break;
          }
          case "pubSub":
            for (const listener of this._listeners) listener(msg);
            break;
        }
      };
      ws.onclose = () => this._drop();
    }

    _drop() {
      const err = new Error("canopee gateway connection closed");
      while (this._pending.length) this._pending.shift().reject(err);
      this._ws = null;
    }
  }

  global.CanopeeWeb = CanopeeWeb;
})(typeof window !== "undefined" ? window : this);