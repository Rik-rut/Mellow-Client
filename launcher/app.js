'use strict';

const tauri = window.__TAURI__;
const invoke = tauri && tauri.core ? tauri.core.invoke.bind(tauri.core) : null;

const params = new URLSearchParams(location.search);

const savedWrap = document.getElementById('saved-wrap');
const savedList = document.getElementById('saved-list');
const discoveredList = document.getElementById('discovered-list');
const input = document.getElementById('server-input');
const connectBtn = document.getElementById('connect-btn');
const connectForm = document.getElementById('connect-form');
const statusEl = document.getElementById('status');
const banner = document.getElementById('unreachable-banner');
const rescanBtn = document.getElementById('rescan-btn');

function setStatus(text, kind) {
  statusEl.textContent = text || '';
  statusEl.className = 'status' + (kind ? ' ' + kind : '');
}

const FALLBACK_LOGO = 'mellow.svg';

function serverLogo(url) {
  return url.replace(/\/+$/, '') + '/mellow.svg';
}

function makeRow(url, name, onOpen, onRemove) {
  const row = document.createElement('div');
  row.className = 'server-row';

  const fav = document.createElement('span');
  fav.className = 'fav';
  const img = document.createElement('img');
  img.alt = '';
  img.src = serverLogo(url);
  img.addEventListener('error', () => { img.src = FALLBACK_LOGO; }, { once: true });
  const dot = document.createElement('span');
  dot.className = 'dot';
  fav.appendChild(img);
  fav.appendChild(dot);

  const host = document.createElement('span');
  host.className = 'host';
  host.textContent = name ? name + ' — ' + url : url;

  row.appendChild(fav);
  row.appendChild(host);

  row.addEventListener('click', () => onOpen(url));

  if (onRemove) {
    const x = document.createElement('button');
    x.className = 'x';
    x.title = 'Forget this server';
    x.textContent = '\u00d7';
    x.addEventListener('click', (e) => {
      e.stopPropagation();
      onRemove(url);
    });
    row.appendChild(x);
  }

  if (invoke) {
    invoke('test_server', { url })
      .then(() => dot.classList.add('ok'))
      .catch(() => dot.classList.add('dead'));
  }
  return row;
}

async function connectTo(url) {
  if (!invoke) {
    setStatus('Run inside the Mellow desktop app.', 'err');
    return;
  }
  setStatus('Connecting…', 'busy');
  connectBtn.disabled = true;
  try {
    await invoke('connect', { url });
    setStatus('Connected.', '');
  } catch (err) {
    setStatus(String(err), 'err');
  } finally {
    connectBtn.disabled = false;
  }
}

connectForm.addEventListener('submit', (e) => {
  e.preventDefault();
  connectTo(input.value.trim());
});

async function loadSaved() {
  if (!invoke) return;
  try {
    const info = await invoke('get_servers');
    if (info.last_unreachable) banner.hidden = false;
    const servers = info.servers || [];
    if (servers.length > 0) {
      savedWrap.hidden = false;
      savedList.innerHTML = '';
      servers.slice().reverse().forEach((url) => {
        savedList.appendChild(
          makeRow(url, null, connectTo, (u) =>
            invoke('forget_server', { url: u }).then(loadSaved)
          )
        );
      });
    }
  } catch (_) {}
}

async function scan() {
  discoveredList.innerHTML = '';
  const busy = document.createElement('div');
  busy.className = 'muted small';
  busy.textContent = 'Scanning…';
  discoveredList.appendChild(busy);
  rescanBtn.disabled = true;
  try {
    const found = invoke ? await invoke('scan_lan') : [];
    discoveredList.innerHTML = '';
    if (!found || found.length === 0) {
      const none = document.createElement('div');
      none.className = 'muted small';
      none.textContent = 'Nothing found — type the address above instead. (Discovery works on the same Wi-Fi/LAN subnet.)';
      discoveredList.appendChild(none);
      return;
    }
    found.forEach((srv) =>
      discoveredList.appendChild(makeRow(srv.url, srv.name, connectTo, null))
    );
  } catch (_) {
    discoveredList.innerHTML = '<div class="muted small">Scan failed — type the address above.</div>';
  } finally {
    rescanBtn.disabled = false;
  }
}

rescanBtn.addEventListener('click', scan);

loadSaved();
scan();
