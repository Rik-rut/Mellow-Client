(function () {
  'use strict';
  // Runs natively on every top-level page. Only act on server pages (http/https);
  // never run on the bundled launcher (tauri.localhost / tauri:// origins).
  if (!/^https?:$/.test(location.protocol)) return;
  if (location.hostname === 'tauri.localhost' || location.host.indexOf('tauri.localhost') !== -1) return;

  var CSS = [
    '#mellow-desktop-back-btn {',
    '  position: absolute;',
    '  top: 20px;',
    '  left: 20px;',
    '  z-index: 99999;',
    '  display: inline-flex;',
    '  align-items: center;',
    '  gap: 8px;',
    '  padding: 8px 16px;',
    '  border-radius: 9999px;',
    '  background: rgba(33, 40, 46, 0.85);',
    '  backdrop-filter: blur(16px);',
    '  -webkit-backdrop-filter: blur(16px);',
    '  border: 1px solid rgba(255, 255, 255, 0.12);',
    '  color: #cdd9e2;',
    '  font-family: inherit;',
    '  font-size: 13px;',
    '  font-weight: 500;',
    '  cursor: pointer;',
    '  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);',
    '  transition: all 0.18s ease;',
    '  user-select: none;',
    '  text-decoration: none;',
    '}',
    '#mellow-desktop-back-btn:hover {',
    '  background: rgba(43, 53, 61, 0.95);',
    '  border-color: rgba(60, 213, 185, 0.45);',
    '  color: #3cd5b9;',
    '  transform: translateY(-1px);',
    '  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45);',
    '}',
    '#mellow-desktop-back-btn:active {',
    '  transform: translateY(0) scale(0.97);',
    '}',
    '#mellow-desktop-back-btn svg {',
    '  width: 16px;',
    '  height: 16px;',
    '  flex-shrink: 0;',
    '  transition: transform 0.18s ease;',
    '}',
    '#mellow-desktop-back-btn:hover svg {',
    '  transform: translateX(-2px);',
    '}'
  ].join('\n');

  function ensureStyles() {
    if (document.getElementById('mellow-desktop-bridge-css')) return;
    var style = document.createElement('style');
    style.id = 'mellow-desktop-bridge-css';
    style.textContent = CSS;
    (document.head || document.documentElement).appendChild(style);
  }

  function inject() {
    try {
      // Ensure any old rail-change-server button is completely removed
      var oldRailBtn = document.getElementById('rail-change-server');
      if (oldRailBtn) oldRailBtn.remove();

      // Only inject on the login page (#auth-screen)
      var authScreen = document.getElementById('auth-screen');
      if (!authScreen) return;

      if (document.getElementById('mellow-desktop-back-btn')) return;

      ensureStyles();

      var btn = document.createElement('button');
      btn.id = 'mellow-desktop-back-btn';
      btn.type = 'button';
      btn.title = 'Back to server list';
      btn.innerHTML =
        '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">' +
        '<line x1="19" y1="12" x2="5" y2="12"></line>' +
        '<polyline points="12 19 5 12 12 5"></polyline>' +
        '</svg>' +
        '<span>Change server</span>';

      btn.addEventListener('click', function (e) {
        e.preventDefault();
        e.stopPropagation();
        window.location.href = 'mellow-desktop://switch-server';
      });

      authScreen.appendChild(btn);
    } catch (e) {}
  }

  window.addEventListener('keydown', function (e) {
    if (e.altKey && e.key === 'ArrowLeft') {
      window.location.href = 'mellow-desktop://switch-server';
    }
  });

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', inject);
  } else {
    inject();
  }
  setInterval(inject, 1500);
})();
