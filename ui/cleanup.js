// 磁盘清理窗口：垃圾清理 / 大文件 / C 盘专清。
// 只通过 Tauri 命令与后端交互，所有路径都由后端复核，界面不拼任意路径。
(function () {
  'use strict';

  var invoke = function (cmd, args) { return window.__TAURI__.core.invoke(cmd, args || {}); };
  var t = function (k, v) { return window.I18N.t(k, v); };
  var isEn = function () { return window.I18N.isEn(); };
  var $ = function (sel) { return document.querySelector(sel); };

  var ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) { return ESC[c]; });
  }
  function fmtSize(n) {
    n = Number(n) || 0;
    if (n <= 0) return '0 B';
    var units = ['B', 'KB', 'MB', 'GB', 'TB'];
    var i = 0;
    var v = n;
    while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
    return (i === 0 ? String(Math.round(v)) : v.toFixed(v >= 100 ? 0 : 1)) + ' ' + units[i];
  }
  function fmtNum(n) {
    return (Number(n) || 0).toLocaleString(isEn() ? 'en-US' : 'zh-CN');
  }
  function pad(n) { return n < 10 ? '0' + n : String(n); }
  function fmtTime(unix) {
    if (!unix) return '';
    var d = new Date(unix * 1000);
    return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) +
      ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes());
  }

  var S = {
    admin: false,
    disks: [],
    tab: 'junk',
    junk: null,
    selected: null,
    expanded: {},
    large: [],
    largeSel: {},
    largeDrives: {},
    cPath: null,
    cParent: null,
    cResult: null,
    cSystem: null,
    componentBytes: null,
    cache: null,
    cacheSel: {},
    apps: [],
    appSizes: {},
    appBusy: {},
    appTotal: 0,
    sysProgress: {},
    busy: {},
  };

  // ---------- 通用 ----------
  var toastTimer = 0;
  function toast(msg, ms) {
    var el = $('#toast');
    el.textContent = msg;
    el.classList.remove('hidden');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () { el.classList.add('hidden'); }, ms || 3200);
  }

  var modalResolve = null;
  function askConfirm(title, body) {
    $('#modalTitle').textContent = title;
    $('#modalBody').textContent = body;
    $('#modal').classList.remove('hidden');
    return new Promise(function (resolve) { modalResolve = resolve; });
  }
  function closeModal(ok) {
    $('#modal').classList.add('hidden');
    var fn = modalResolve;
    modalResolve = null;
    if (fn) fn(!!ok);
  }

  function setIndet(sel, on) {
    var fill = document.querySelector(sel);
    if (!fill) return;
    var bar = fill.parentElement;
    if (bar) bar.classList.toggle('indet', !!on);
    if (!on) fill.style.width = '';
  }
  function finishBar(sel) {
    var fill = document.querySelector(sel);
    if (!fill) return;
    if (fill.parentElement) fill.parentElement.classList.remove('indet');
    fill.style.width = '100%';
  }

  // ---------- 磁盘 ----------
  function renderDisks() {
    var chips = function (container, multi, onToggle) {
      container.innerHTML = S.disks.map(function (d) {
        var active = multi ? !!S.largeDrives[d.letter] : true;
        return '<button class="disk-chip' + (active ? ' active' : '') + '" data-letter="' + esc(d.letter) + '">' +
          '<span class="dot"></span><b>' + esc(d.letter) + '</b>' +
          '<span class="free">' + fmtSize(d.free) + ' / ' + fmtSize(d.total) + '</span></button>';
      }).join('');
      container.querySelectorAll('.disk-chip').forEach(function (el) {
        el.addEventListener('click', function () {
          if (!multi) return;
          var letter = el.getAttribute('data-letter');
          if (S.largeDrives[letter]) delete S.largeDrives[letter];
          else S.largeDrives[letter] = true;
          renderDisks();
          onToggle && onToggle();
        });
      });
    };
    chips($('#junkDisks'), false);
    chips($('#largeDisks'), true);
  }

  function fillTargetDrives() {
    var sel = $('#cTargetDrive');
    var prev = sel.value;
    var opts = S.disks.filter(function (d) { return d.letter.toUpperCase() !== 'C:'; });
    if (!opts.length) {
      sel.innerHTML = '<option value="">—</option>';
      return;
    }
    sel.innerHTML = opts.map(function (d) {
      return '<option value="' + esc(d.letter) + '">' + esc(d.letter) + ' · ' + fmtSize(d.free) + '</option>';
    }).join('');
    var keep = Array.prototype.some.call(sel.options, function (o) { return o.value === prev; });
    if (keep) sel.value = prev;
  }

  // ---------- 标签页 ----------
  function activateTab(tab, force) {
    tab = tab === 'large' || tab === 'cdrive' ? tab : 'junk';
    if (S.tab === tab && !force) return;
    S.tab = tab;
    document.querySelectorAll('.tab').forEach(function (el) {
      el.classList.toggle('active', el.getAttribute('data-tab') === tab);
    });
    $('#panelJunk').classList.toggle('hidden', tab !== 'junk');
    $('#panelLarge').classList.toggle('hidden', tab !== 'large');
    $('#panelC').classList.toggle('hidden', tab !== 'cdrive');
    if (tab === 'cdrive') renderC();
  }

  document.querySelectorAll('.tab').forEach(function (el) {
    el.addEventListener('click', function () { activateTab(el.getAttribute('data-tab'), true); });
  });

  // ---------- 垃圾清理 ----------
  function junkRow(c) {
    var key = 'disk.cat.' + c.id;
    var title = t(key);
    if (title === key) title = c.id;
    var desc = t(key + '.desc');
    if (desc === key + '.desc') desc = '';
    var disabled = c.needsAdmin && !S.admin;
    var checked = !!S.selected && !!S.selected[c.id] && !disabled;
    var badges = '';
    if (c.needsAdmin) badges += '<span class="badge small warn">' + esc(t('disk.badge.admin')) + '</span>';
    if (c.needsConfirm) badges += '<span class="badge small danger">' + esc(t('disk.badge.confirm')) + '</span>';
    var count = c.items > 0 ? '<span class="cat-count">' + esc(t('disk.junk.files', { n: fmtNum(c.items) })) + '</span>' : '';
    var expanded = !!S.expanded[c.id];
    var entries = '';
    if (expanded) {
      var list = (c.entries || []).slice(0, 40).map(function (e) {
        return '<div class="entry-row"><span class="en" title="' + esc(e.path) + '">' +
          esc(e.label || e.path) + '</span><span class="es">' + fmtSize(e.size) + '</span>' +
          (e.detail ? '<span class="ed">' + esc(e.detail) + '</span>' : '') + '</div>';
      }).join('');
      entries = '<div class="cat-entries">' +
        (list || '<div class="entry-row"><span class="en">' + esc(c.note || '—') + '</span></div>') + '</div>';
    }
    var noteKey = {
      drivers: 'disk.junk.driverNote',
      leftovers: 'disk.junk.leftoverNote',
      registry: 'disk.junk.registryNote',
      recycle: 'disk.junk.recycleNote',
    }[c.id];
    var note = c.note || (noteKey ? t(noteKey) : '');
    return '<div class="cat" data-id="' + esc(c.id) + '">' +
      '<div class="cat-row">' +
      '<input type="checkbox" class="cat-check" data-id="' + esc(c.id) + '"' +
      (checked ? ' checked' : '') + (disabled ? ' disabled' : '') + '>' +
      '<div class="cat-main"><div class="cat-title">' + esc(title) + badges + '</div>' +
      (desc ? '<div class="cat-desc">' + esc(desc) + '</div>' : '') + '</div>' +
      '<div class="cat-size' + (c.bytes ? '' : ' zero') + '">' + fmtSize(c.bytes) + count + '</div>' +
      (c.entries && c.entries.length ? '<button class="link cat-more" data-id="' + esc(c.id) + '">' +
        esc(expanded ? t('common.hide') : t('disk.junk.entries')) + '</button>' : '') +
      '</div>' +
      (note ? '<div class="cat-note">' + esc(note) + '</div>' : '') +
      entries +
      '</div>';
  }

  function renderJunk() {
    var report = S.junk;
    if (!report) return;
    if (!S.selected) {
      S.selected = {};
      report.categories.forEach(function (c) {
        if (c.defaultOn && c.bytes > 0 && !(c.needsAdmin && !S.admin)) S.selected[c.id] = true;
      });
    }
    $('#junkList').innerHTML = report.categories.map(junkRow).join('') ||
      '<div class="empty">' + esc(t('disk.emptyHint')) + '</div>';

    $('#junkList').querySelectorAll('.cat-check').forEach(function (el) {
      el.addEventListener('change', function () {
        var id = el.getAttribute('data-id');
        if (el.checked) S.selected[id] = true;
        else delete S.selected[id];
        syncJunkSummary();
      });
    });
    $('#junkList').querySelectorAll('.cat-more').forEach(function (el) {
      el.addEventListener('click', function () {
        var id = el.getAttribute('data-id');
        if (S.expanded[id]) delete S.expanded[id];
        else S.expanded[id] = true;
        renderJunk();
      });
    });
    syncJunkSummary();
  }

  function junkCats(id) {
    if (!S.junk) return [];
    return S.junk.categories.filter(function (c) { return c.id === id; });
  }

  function syncJunkSummary() {
    var cats = S.junk ? S.junk.categories : [];
    var sel = S.selected || {};
    var count = 0;
    var bytes = 0;
    cats.forEach(function (c) {
      if (sel[c.id]) { count++; bytes += c.bytes; }
    });
    $('#junkSummary').textContent = count
      ? t('disk.junk.selected', { n: count, size: fmtSize(bytes) })
      : t('disk.junk.none');
    var enabled = cats.filter(function (c) { return !(c.needsAdmin && !S.admin); });
    var allOn = enabled.length > 0 && enabled.every(function (c) { return !!sel[c.id]; });
    $('#junkAll').checked = allOn;
    $('#junkAll').indeterminate = !allOn && enabled.some(function (c) { return !!sel[c.id]; });
    $('#junkClean').disabled = count === 0 || !!S.busy.junk;
  }

  async function scanJunk() {
    if (S.busy.junk) return;
    S.busy.junk = true;
    S.selected = null;
    $('#junkClean').disabled = true;
    $('#junkScan').disabled = true;
    $('#junkCancel').classList.remove('hidden');
    $('#junkMeta').textContent = t('disk.working');
    setIndet('#junkBar', true);
    try {
      var report = await invoke('junk_scan');
      S.junk = report;
      S.expanded = {};
      renderJunk();
      $('#junkMeta').textContent = t('disk.junk.scanDone', { n: fmtNum(report.filesScanned) });
      $('#junkNotice').classList.toggle('hidden', !!report.admin);
      $('#junkNotice').textContent = report.admin ? '' : t('disk.elevateHint');
    } catch (e) {
      $('#junkMeta').textContent = '';
      toast(t('disk.toast.failed', { err: String(e) }), 5000);
    } finally {
      S.busy.junk = false;
      $('#junkScan').disabled = false;
      $('#junkCancel').classList.add('hidden');
      finishBar('#junkBar');
      syncJunkSummary();
    }
  }

  async function cleanJunk() {
    var ids = Object.keys(S.selected).filter(function (k) { return !!S.selected[k]; });
    if (!ids.length) { toast(t('disk.toast.noSelection')); return; }
    var cats = ids.map(function (id) { return junkCats(id)[0]; }).filter(Boolean);
    if (cats.some(function (c) { return c.needsAdmin; }) && !S.admin) {
      toast(t('disk.needAdmin'), 4200);
      return;
    }
    var bytes = cats.reduce(function (a, c) { return a + c.bytes; }, 0);
    var lines = cats.map(function (c) {
      var title = t('disk.cat.' + c.id);
      return '· ' + (title === 'disk.cat.' + c.id ? c.id : title) + '  ' + fmtSize(c.bytes);
    }).join('\n');
    var body = t('disk.confirmClean', { n: cats.length, size: fmtSize(bytes) }) + '\n' + lines;
    var recycle = cats.filter(function (c) { return c.id === 'recycle'; })[0];
    if (recycle) {
      body += '\n\n' + t('disk.confirmRecycle', { size: fmtSize(recycle.bytes), n: fmtNum(recycle.items) });
    }
    if (!(await askConfirm(t('disk.confirmTitle'), body))) return;

    S.busy.junk = true;
    $('#junkClean').disabled = true;
    $('#junkMeta').textContent = t('disk.cleaning');
    setIndet('#junkBar', true);
    try {
      var res = await invoke('junk_clean', { ids: ids, confirm: true });
      var msg = t('disk.toast.cleanDone', { size: fmtSize(res.freed) });
      if (res.failed) msg += ' · ' + t('disk.resultFail', { n: res.failed });
      toast(msg, 4200);
      S.busy.junk = false;
      await scanJunk();
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 5200);
    } finally {
      S.busy.junk = false;
      finishBar('#junkBar');
      syncJunkSummary();
    }
  }

  // ---------- 大文件 ----------
  var LARGE_LIMIT = 500;

  function renderLarge() {
    var list = S.large;
    if (!list.length) {
      $('#largeList').innerHTML = '<div class="empty">' +
        esc(t('disk.large.empty', { size: $('#largeMin').value + ' MB' })) + '</div>';
      $('#largeSummary').textContent = '—';
      $('#largeDelete').disabled = true;
      return;
    }
    var shown = list.slice(0, LARGE_LIMIT);
    var html = shown.map(function (f) {
      return '<div class="row-file">' +
        '<input type="checkbox" class="file-check" data-path="' + esc(f.path) + '"' +
        (S.largeSel[f.path] ? ' checked' : '') + '>' +
        '<div class="file-main"><div class="file-name" title="' + esc(f.path) + '">' + esc(f.name) + '</div>' +
        '<div class="file-dir" title="' + esc(f.dir) + '">' + esc(f.dir) + '</div></div>' +
        '<div class="file-size">' + fmtSize(f.size) + '</div>' +
        '<div class="file-time">' + esc(fmtTime(f.modified)) + '</div>' +
        '<div class="file-op"><button class="link reveal" data-path="' + esc(f.path) + '">' +
        esc(t('disk.reveal')) + '</button><button class="link danger row-del" data-path="' +
        esc(f.path) + '">' + esc(t('disk.large.deleteOne')) + '</button></div></div>';
    }).join('');
    if (list.length > LARGE_LIMIT) {
      html += '<div class="empty">' + esc(t('disk.large.limit', { n: LARGE_LIMIT })) + '</div>';
    }
    $('#largeList').innerHTML = html;
    $('#largeList').querySelectorAll('.file-check').forEach(function (el) {
      el.addEventListener('change', function () {
        var p = el.getAttribute('data-path');
        if (el.checked) S.largeSel[p] = true;
        else delete S.largeSel[p];
        syncLargeSummary();
      });
    });
    $('#largeList').querySelectorAll('.reveal').forEach(function (el) {
      el.addEventListener('click', function () {
        invoke('cleanup_reveal', { path: el.getAttribute('data-path') }).catch(function (e) {
          toast(t('disk.toast.failed', { err: String(e) }));
        });
      });
    });
    $('#largeList').querySelectorAll('.row-del').forEach(function (el) {
      el.addEventListener('click', function () { deleteLargeOne(el.getAttribute('data-path')); });
    });
    syncLargeSummary();
  }

  function syncLargeSummary() {
    var paths = Object.keys(S.largeSel).filter(function (k) { return !!S.largeSel[k]; });
    var byPath = {};
    S.large.forEach(function (f) { byPath[f.path] = f.size; });
    var bytes = paths.reduce(function (a, p) { return a + (byPath[p] || 0); }, 0);
    $('#largeSummary').textContent = paths.length
      ? t('disk.large.selected', { n: paths.length, size: fmtSize(bytes) })
      : '—';
    $('#largeDelete').disabled = paths.length === 0 || !!S.busy.large;
    var visible = S.large.slice(0, LARGE_LIMIT);
    var checkedVisible = visible.filter(function (f) { return S.largeSel[f.path]; }).length;
    $('#largeAll').checked = visible.length > 0 && checkedVisible === visible.length;
    $('#largeAll').indeterminate = checkedVisible > 0 && !$('#largeAll').checked;
  }

  async function scanLarge() {
    if (S.busy.large) return;
    var drives = Object.keys(S.largeDrives).filter(function (k) { return !!S.largeDrives[k]; });
    if (!drives.length) { toast(t('disk.toast.noSelection')); return; }
    S.busy.large = true;
    S.largeSel = {};
    $('#largeScan').disabled = true;
    $('#largeCancel').classList.remove('hidden');
    $('#largeMeta').textContent = t('disk.working');
    setIndet('#largeBar', true);
    try {
      var list = await invoke('large_scan', { drives: drives, minMb: Number($('#largeMin').value) });
      S.large = list || [];
      renderLarge();
      $('#largeMeta').textContent = list.length
        ? t('disk.progress.files', { n: fmtNum(list.length), size: fmtSize(list.reduce(function (a, f) { return a + f.size; }, 0)) })
        : '';
    } catch (e) {
      $('#largeMeta').textContent = '';
      toast(t('disk.toast.failed', { err: String(e) }), 5200);
    } finally {
      S.busy.large = false;
      $('#largeScan').disabled = false;
      $('#largeCancel').classList.add('hidden');
      finishBar('#largeBar');
      syncLargeSummary();
    }
  }

  async function deleteLargeOne(path) {
    if (S.busy.large) return;
    var f = S.large.filter(function (x) { return x.path === path; })[0];
    var body = t('disk.large.confirmOne', { size: f ? fmtSize(f.size) : '' }) + '\n' + path;
    if (!(await askConfirm(t('disk.confirmTitle'), body))) return;

    S.busy.large = true;
    setIndet('#largeBar', true);
    $('#largeMeta').textContent = t('disk.cleaning');
    try {
      var res = await invoke('large_delete', { paths: [path] });
      var msg = t('disk.toast.cleanDone', { size: fmtSize(res.freed) });
      if (res.failed) msg += ' · ' + t('disk.resultFail', { n: res.failed });
      toast(msg, 4200);
      if (!res.failed) {
        S.large = S.large.filter(function (x) { return x.path !== path; });
        delete S.largeSel[path];
        renderLarge();
      }
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 5200);
    } finally {
      S.busy.large = false;
      finishBar('#largeBar');
      syncLargeSummary();
    }
  }

  async function deleteLarge() {
    var paths = Object.keys(S.largeSel).filter(function (k) { return !!S.largeSel[k]; });
    if (!paths.length) { toast(t('disk.toast.noSelection')); return; }
    var byPath = {};
    S.large.forEach(function (f) { byPath[f.path] = f.size; });
    var bytes = paths.reduce(function (a, p) { return a + (byPath[p] || 0); }, 0);
    var sample = paths.slice(0, 8).map(function (p) { return '· ' + p; }).join('\n');
    if (paths.length > 8) sample += '\n…';
    var body = t('disk.large.confirm', { n: paths.length, size: fmtSize(bytes) }) + '\n' + sample;
    if (!(await askConfirm(t('disk.confirmTitle'), body))) return;

    S.busy.large = true;
    $('#largeDelete').disabled = true;
    setIndet('#largeBar', true);
    $('#largeMeta').textContent = t('disk.cleaning');
    try {
      var res = await invoke('large_delete', { paths: paths });
      var msg = t('disk.toast.cleanDone', { size: fmtSize(res.freed) });
      if (res.failed) msg += ' · ' + t('disk.resultFail', { n: res.failed });
      toast(msg, 4200);
      S.large = S.large.filter(function (f) { return !S.largeSel[f.path]; });
      S.largeSel = {};
      renderLarge();
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 5200);
    } finally {
      S.busy.large = false;
      finishBar('#largeBar');
      syncLargeSummary();
    }
  }

  // ---------- C 盘 ----------
  function renderC() {
    var c = S.disks.filter(function (d) { return d.letter.toUpperCase() === 'C:'; })[0];
    if (c) {
      var used = c.total - c.free;
      var pct = c.total ? Math.round(used / c.total * 100) : 0;
      var bar = $('#cBar');
      if (bar) {
        if (bar.parentElement) bar.parentElement.classList.remove('indet');
        bar.style.width = pct + '%';
      }
      $('#cFree').textContent = t('disk.c.free', { size: fmtSize(c.free), total: fmtSize(c.total) });
    }
    fillTargetDrives();
    if (S.cResult) renderGroups();
    if (S.cSystem) renderSystem();
    renderCache();
    if (S.apps.length) renderApps();
  }

  function renderGroups() {
    var r = S.cResult;
    if (!r) return;
    $('#cGroupsWrap').classList.remove('hidden');
    $('#cPath').textContent = r.path;
    $('#cUp').classList.toggle('hidden', !S.cParent);
    var groups = r.groups || [];
    var max = groups.length ? groups[0].size : 0;
    var total = r.total || 0;
    $('#cMeta').textContent = t('disk.c.total', { size: fmtSize(total) }) +
      (r.canceled ? ' · ' + t('disk.canceled') : '');
    $('#cGroups').innerHTML = groups.length ? groups.map(function (g) {
      var pct = max ? Math.max(2, Math.round(g.size / max * 100)) : 0;
      var rootFiles = !!g.rootFiles;
      var label = rootFiles ? t('disk.c.rootFiles') : (g.name || g.path);
      return '<div class="group-row' + (rootFiles ? ' static' : '') + '" data-path="' + esc(g.path) + '">' +
        '<span class="group-name" title="' + esc(rootFiles ? label : g.path) + '">' + esc(label) + '</span>' +
        '<span class="group-bar"><i style="width:' + pct + '%"></i></span>' +
        '<span class="group-size">' + fmtSize(g.size) + '</span>' +
        '<span class="group-files">' + esc(t('disk.c.files', { n: fmtNum(g.files) })) + '</span></div>';
    }).join('') : '<div class="empty">' + esc(t('disk.c.emptyGroup')) + '</div>';
    $('#cGroups').querySelectorAll('.group-row').forEach(function (el) {
      if (el.classList.contains('static')) return;
      el.addEventListener('click', function () { analyzeC(el.getAttribute('data-path')); });
    });
  }

  async function analyzeC(path) {
    if (S.busy.cdrive) return;
    S.busy.cdrive = true;
    $('#cAnalyze').disabled = true;
    $('#cCancel').classList.remove('hidden');
    $('#cMeta').textContent = t('disk.c.analyzing', { path: path || 'C:\\' });
    setIndet('#cBar', true);
    try {
      var res = await invoke('cdrive_analyze', path ? { path: path } : {});
      S.cResult = res;
      S.cPath = res.path;
      S.cParent = res.parent;
      renderGroups();
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 5200);
    } finally {
      S.busy.cdrive = false;
      $('#cAnalyze').disabled = false;
      $('#cCancel').classList.add('hidden');
      setIndet('#cBar', false);
      renderC();
    }
  }

  function renderSystem() {
    var r = S.cSystem;
    if (!r) return;
    $('#cReclaim').textContent = t('disk.c.reclaimable', { size: fmtSize(r.reclaimable) });
    $('#cSystem').innerHTML = r.items.map(function (item) {
      var name = t('disk.c.sys.' + item.id);
      if (name === 'disk.c.sys.' + item.id) name = item.id;
      var descKey = 'disk.c.sys.' + item.id + '.desc';
      var desc = t(descKey);
      if (desc === descKey) desc = item.detail || '';
      if (item.id === 'winsxs') {
        if (item.detail === 'admin') desc = t('disk.c.componentAdmin');
        else if (item.detail === 'dism_failed') desc = t('disk.c.componentFailed');
        else if (item.detail === 'dism_analyzed') desc = t('disk.c.componentMeasured');
      }
      var measuring = item.id === 'winsxs' && (!!S.busy.component || item.detail === 'dism');
      var sizeText;
      if (measuring) sizeText = t('disk.c.componentAnalyzing');
      else if (item.bytes > 0) sizeText = fmtSize(item.bytes);
      else if (item.detail === 'vss' || item.id === 'winsxs') sizeText = t('disk.c.notMeasured');
      else sizeText = fmtSize(0);
      var needAdmin = item.id !== 'recycle' && item.id !== 'pagefile';
      var canRun = item.id !== 'pagefile' && (item.available || item.id === 'restore' || item.id === 'winsxs');
      var sp = S.sysProgress[item.id];
      var pct = sp && sp.total > 0 ? Math.min(100, Math.round((sp.bytes || 0) * 100 / sp.total)) : null;
      var prog = '';
      if (sp) {
        prog = '<div class="sys-prog' + (pct == null ? ' indet' : '') + '">' +
          '<div class="progbar"><i style="width:' + (pct == null ? 35 : pct) + '%"></i></div>' +
          '<span>' + esc(pct == null ? t('disk.c.sysRunning') : t('disk.c.sysPercent', { pct: pct })) + '</span></div>';
      }
      var btn = canRun ? '<button class="btn ghost sys-run" data-id="' + esc(item.id) + '"' +
        (S.busy.system ? ' disabled' : '') + '>' + esc(t('disk.c.run')) + '</button>' : '';
      if (needAdmin && !S.admin && canRun) {
        btn = '<button class="btn ghost sys-run" data-id="' + esc(item.id) + '" data-admin="1"' +
          (S.busy.system ? ' disabled' : '') + '>' + esc(t('disk.c.run')) + '</button>';
      }
      return '<div class="sys-card"><div class="sys-title"><span>' + esc(name) + '</span>' +
        (needAdmin && !S.admin ? '<span class="badge small warn">' + esc(t('disk.badge.admin')) + '</span>' : '') +
        '</div><div class="sys-size">' + sizeText + '</div>' +
        (desc ? '<div class="sys-desc">' + esc(desc) + '</div>' : '') + prog +
        '<div class="sys-act">' + btn + '</div></div>';
    }).join('');
    $('#cSystem').querySelectorAll('.sys-run').forEach(function (el) {
      el.addEventListener('click', function () { runSystemAction(el.getAttribute('data-id')); });
    });
  }

  var CACHE_BUCKET_IDS = ['d7', 'd30', 'd90', 'd180', 'old'];

  function cacheText(id, suffix) {
    var key = 'disk.cache.cat.' + id + (suffix || '');
    var val = t(key);
    return val === key ? '' : val;
  }

  function cacheAppText(app) {
    var key = 'disk.cache.app.' + app;
    var val = t(key);
    return val === key ? app : val;
  }

  function cacheItemText(cat) {
    var name = cacheText(cat.id) || cat.id;
    var idx = name.indexOf(' · ');
    return idx >= 0 ? name.slice(idx + 3) : name;
  }

  function cacheBucketText(id) {
    var key = 'disk.cache.bucket.' + id;
    var val = t(key);
    return val === key ? id : val;
  }

  function cacheBucketTip(id) {
    var key = 'disk.cache.bucket.' + id + '.tip';
    var val = t(key);
    return val === key ? cacheBucketText(id) : val;
  }

  function cacheBucketOf(cat, id) {
    var list = cat.buckets || [];
    for (var i = 0; i < list.length; i++) {
      if (list[i].id === id) return list[i];
    }
    return null;
  }

  function renderCache() {
    var report = S.cache;
    if (!report) {
      $('#cCacheMeta').textContent = S.busy.cache ? t('disk.cache.scanning') : '';
      $('#cCacheList').innerHTML = '<div class="empty">' + esc(t('disk.cache.emptyHint')) + '</div>';
      $('#cCacheSummary').textContent = '—';
      $('#cCacheClean').disabled = true;
      return;
    }
    var cats = report.categories || [];
    if (!cats.length) {
      $('#cCacheMeta').textContent = t('disk.cache.meta', {
        apps: 0, size: fmtSize(0), files: fmtNum(0)
      });
      $('#cCacheList').innerHTML = '<div class="empty">' + esc(t('disk.cache.none')) + '</div>';
      syncCacheSummary();
      return;
    }

    var groups = [];
    var groupMap = {};
    cats.forEach(function (c) {
      if (!groupMap[c.app]) {
        groupMap[c.app] = { app: c.app, cats: [] };
        groups.push(groupMap[c.app]);
      }
      groupMap[c.app].cats.push(c);
    });

    var head = '<div class="cache-head"><span class="cache-head-name">' +
      esc(t('disk.cache.item')) + '</span>' + CACHE_BUCKET_IDS.map(function (id) {
        return '<span title="' + esc(cacheBucketTip(id)) + '">' + esc(cacheBucketText(id)) + '</span>';
      }).join('') + '</div>';

    var body = groups.map(function (g) {
      var bytes = g.cats.reduce(function (a, c) { return a + (c.bytes || 0); }, 0);
      var files = g.cats.reduce(function (a, c) { return a + (c.files || 0); }, 0);
      var appHead = '<div class="cache-app"><span class="cache-app-name">' +
        esc(cacheAppText(g.app)) + '</span><span class="cache-app-meta">' +
        esc(t('disk.cache.appMeta', { n: g.cats.length, size: fmtSize(bytes), files: fmtNum(files) })) +
        '</span></div>';
      var rows = g.cats.map(function (c) {
        var name = cacheItemText(c);
        var desc = cacheText(c.id, '.desc');
        var sel = S.cacheSel[c.id] || {};
        // 本次运行里被清理过的分类：行上标「已清理」，总量位置也换成「已清理」
        var cleaned = (c.buckets || []).some(function (b) { return b && b.cleaned; });
        var cells = CACHE_BUCKET_IDS.map(function (id) {
          var b = cacheBucketOf(c, id);
          var bytes = b ? (b.bytes || 0) : 0;
          var checked = !!(sel[id] && bytes > 0);
          var empty = !bytes;
          return '<label class="cache-cell' + (empty ? ' zero' : '') + '" title="' +
            esc(cacheBucketTip(id)) + '">' +
            '<input type="checkbox" data-cat="' + esc(c.id) + '" data-bucket="' + esc(id) + '"' +
            (checked ? ' checked' : '') + (empty ? ' disabled' : '') + '>' +
            '<span>' + esc(bytes ? fmtSize(bytes) : '—') + '</span></label>';
        }).join('');
        var rowTotal = c.files
          ? t('disk.cache.rowTotal', { size: fmtSize(c.bytes || 0), files: fmtNum(c.files || 0) })
          : (cleaned ? t('disk.cache.cleaned') : t('disk.cache.rowEmpty'));
        var tip = (c.entries || []).map(function (e) { return e.path; }).join('\n');
        return '<div class="cache-row' + (c.risky ? ' risky' : '') + (c.files ? '' : ' zero') +
          '" title="' + esc(tip) + '">' +
          '<div class="cache-main"><div class="cache-name-line"><span class="cache-name">' +
          esc(name) + '</span>' + (c.risky ? '<span class="badge danger">' +
          esc(t('disk.cache.risky')) + '</span>' : '<span class="badge small">' +
          esc(t('disk.cache.safe')) + '</span>') +
          (cleaned ? '<span class="badge small">' + esc(t('disk.cache.cleaned')) + '</span>' : '') +
          '</div>' +
          (desc ? '<div class="cache-desc">' + esc(desc) + '</div>' : '') +
          '<div class="cache-total">' + esc(rowTotal) + '</div></div>' +
          cells + '</div>';
      }).join('');
      return appHead + rows;
    }).join('');

    $('#cCacheList').innerHTML = head + body;
    $('#cCacheList').querySelectorAll('input[data-cat]').forEach(function (el) {
      el.addEventListener('change', function () {
        var catId = el.getAttribute('data-cat');
        var bucketId = el.getAttribute('data-bucket');
        if (!S.cacheSel[catId]) S.cacheSel[catId] = {};
        if (el.checked) S.cacheSel[catId][bucketId] = true;
        else delete S.cacheSel[catId][bucketId];
        if (!Object.keys(S.cacheSel[catId]).length) delete S.cacheSel[catId];
        syncCacheSummary();
      });
    });
    var bytes = cats.reduce(function (a, c) { return a + (c.bytes || 0); }, 0);
    var files = cats.reduce(function (a, c) { return a + (c.files || 0); }, 0);
    $('#cCacheMeta').textContent = t('disk.cache.meta', {
      apps: groups.length, size: fmtSize(bytes), files: fmtNum(files)
    }) + (report.canceled ? ' · ' + t('disk.canceled') : '');
    syncCacheSummary();
  }

  function syncCacheSummary() {
    var cats = S.cache ? (S.cache.categories || []) : [];
    var n = 0;
    var bytes = 0;
    var files = 0;
    cats.forEach(function (c) {
      var sel = S.cacheSel[c.id] || {};
      CACHE_BUCKET_IDS.forEach(function (id) {
        var b = cacheBucketOf(c, id);
        if (sel[id] && b && b.bytes > 0) {
          n += 1;
          bytes += b.bytes || 0;
          files += b.files || 0;
        }
      });
    });
    $('#cCacheSummary').textContent = n
      ? t('disk.cache.selected', { n: n, size: fmtSize(bytes), files: fmtNum(files) })
      : t('disk.cache.noneSelected');
    $('#cCacheClean').disabled = !n || !!S.busy.cache;
  }

  async function scanCache() {
    if (S.busy.cache) return;
    S.busy.cache = true;
    S.cache = null;
    S.cacheSel = {};
    $('#cCacheClean').disabled = true;
    $('#cCacheScan').disabled = true;
    $('#cCancel').classList.remove('hidden');
    renderCache();
    try {
      var report = await invoke('cache_scan');
      S.cache = report;
      (report.categories || []).forEach(function (c) {
        if (!c.defaultOn || c.risky) return;
        var sel = {};
        (c.buckets || []).forEach(function (b) {
          if (b.bytes > 0) sel[b.id] = true;
        });
        if (Object.keys(sel).length) S.cacheSel[c.id] = sel;
      });
      renderCache();
    } catch (e) {
      S.cache = null;
      renderCache();
      toast(t('disk.toast.failed', { err: String(e) }), 5200);
    } finally {
      S.busy.cache = false;
      $('#cCacheScan').disabled = false;
      if (!S.busy.cdrive) $('#cCancel').classList.add('hidden');
      renderCache();
      syncCacheSummary();
    }
  }

  // 清理成功后把「真正清干净」的桶归零并打标记：列表、行内数字与
  // 「共 N 个应用 · X · Y 个文件」保持自洽，同时保留整份扫描结果。
  // failedItems 里出现的路径所属的桶保留原数据（文件被占用没删掉），
  // 免得把没释放的体积也报成已清理。
  function markCacheCleaned(picked, failedItems, failedCount) {
    if (!S.cache) return;
    var failed = (failedItems || []).map(function (f) {
      return String(f || '').toLowerCase();
    }).filter(Boolean);
    picked.forEach(function (p) {
      var c = (S.cache.categories || []).filter(function (x) {
        return x.id === p.cat.id;
      })[0];
      if (!c) return;
      p.ids.forEach(function (id) {
        var b = cacheBucketOf(c, id);
        if (!b) return;
        if (failed.length) {
          var hit = (b.entries || []).some(function (e) {
            var pre = String(e.path || '').toLowerCase();
            if (!pre) return false;
            return failed.some(function (f) {
              return f === pre || f.indexOf(pre + '\\') === 0 || f.indexOf(pre + '/') === 0;
            });
          });
          if (hit) return;
        } else if (failedCount) {
          return; // 只知道有失败、不知道落哪个桶：保守起来，整批都不动
        }
        b.bytes = 0;
        b.files = 0;
        b.cleaned = true;
      });
      c.bytes = (c.buckets || []).reduce(function (a, b) { return a + (b.bytes || 0); }, 0);
      c.files = (c.buckets || []).reduce(function (a, b) { return a + (b.files || 0); }, 0);
    });
  }

  async function cleanCache() {
    if (S.busy.cache || !S.cache) return;
    var selections = [];
    var picked = [];
    var bytes = 0;
    var files = 0;
    (S.cache.categories || []).forEach(function (c) {
      var sel = S.cacheSel[c.id] || {};
      var ids = CACHE_BUCKET_IDS.filter(function (id) {
        var b = cacheBucketOf(c, id);
        return !!sel[id] && !!b && b.bytes > 0;
      });
      if (!ids.length) return;
      selections.push({ id: c.id, buckets: ids });
      picked.push({ cat: c, ids: ids });
      ids.forEach(function (id) {
        var b = cacheBucketOf(c, id);
        bytes += b.bytes || 0;
        files += b.files || 0;
      });
    });
    if (!selections.length) { toast(t('disk.toast.noSelection')); return; }

    var lines = picked.slice(0, 10).map(function (p) {
      var parts = p.ids.map(function (id) {
        var b = cacheBucketOf(p.cat, id);
        return cacheBucketText(id) + ' ' + fmtSize(b ? b.bytes : 0);
      }).join('、');
      return '· ' + cacheAppText(p.cat.app) + ' · ' + cacheItemText(p.cat) + '：' + parts;
    }).join('\n');
    if (picked.length > 10) lines += '\n· …';
    var body = t('disk.cache.confirmBody', {
      n: selections.length, size: fmtSize(bytes), files: fmtNum(files)
    }) + '\n' + lines;
    if (picked.some(function (p) { return p.cat.risky; })) {
      body += '\n\n' + t('disk.cache.riskyConfirm');
    }
    if (!(await askConfirm(t('disk.cache.confirmTitle'), body))) return;

    S.busy.cache = true;
    $('#cCacheClean').disabled = true;
    $('#cCacheScan').disabled = true;
    $('#cCancel').classList.remove('hidden');
    $('#cCacheMeta').textContent = t('disk.cache.cleaning');
    try {
      var res = await invoke('cache_clean', { selections: selections, confirm: true });
      // 清理后不要清空扫描结果（S.cache = null 会把整块回到初始态，像功能重置了）。
      // 列表继续展示，只有用户点「扫描软件缓存」才重新取数；这里把清干净的桶归零，
      // 让勾选状态、模块总量和行内数字都跟着变。
      markCacheCleaned(picked, res.failedItems || [], res.failed || 0);
      S.cacheSel = {};
      var msg = t('disk.cache.cleanDone', { size: fmtSize(res.freed), n: fmtNum(res.deleted) });
      if ((res.note || '').indexOf('cancelled') >= 0) msg += ' · ' + t('disk.canceled');
      if (res.failed) msg += ' · ' + t('disk.toast.failedCount', { n: fmtNum(res.failed) });
      toast(msg, res.failed ? 5600 : 3600);
      if (res.failedItems && res.failedItems.length) {
        console.warn('cache clean failed:', res.failedItems);
      }
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 5600);
    } finally {
      S.busy.cache = false;
      $('#cCacheScan').disabled = false;
      if (!S.busy.cdrive) $('#cCancel').classList.add('hidden');
      renderCache();
      syncCacheSummary();
    }
  }

  async function measureComponent() {
    if (!S.cSystem || !S.admin || S.busy.component) return;
    S.busy.component = true;
    renderSystem();
    var item = (S.cSystem.items || []).filter(function (i) { return i.id === 'winsxs'; })[0];
    try {
      var bytes = await invoke('cdrive_component_size');
      S.componentBytes = Number(bytes) || 0;
      if (item) { item.bytes = S.componentBytes; item.detail = 'dism_analyzed'; }
    } catch (e) {
      if (item) { item.bytes = 0; item.detail = 'dism_failed'; }
    } finally {
      S.busy.component = false;
      renderSystem();
    }
  }

  async function loadSystem() {
    if (S.busy.system) return;
    S.busy.system = true;
    $('#cSystemScan').disabled = true;
    setIndet('#cBar', true);
    try {
      S.cSystem = await invoke('cdrive_system');
      var win = (S.cSystem.items || []).filter(function (i) { return i.id === 'winsxs'; })[0];
      if (win && S.componentBytes !== null) {
        win.bytes = S.componentBytes;
        win.detail = 'dism_analyzed';
      }
      renderSystem();
      renderC();
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 5200);
    } finally {
      S.busy.system = false;
      $('#cSystemScan').disabled = false;
      setIndet('#cBar', false);
      renderC();
    }
    if (S.admin && S.componentBytes === null) setTimeout(measureComponent, 0);
  }

  async function runSystemAction(id) {
    if (!id) return;
    if (id !== 'recycle' && !S.admin) {
      toast(t('disk.toast.needAdmin'), 4200);
      return;
    }
    var name = t('disk.c.sys.' + id);
    if (!(await askConfirm(t('disk.confirmTitle'), t('disk.c.confirmSys', { name: name })))) return;
    if (id === 'winsxs') toast(t('disk.c.dismHint'), 6000);
    S.busy.system = true;
    S.sysProgress[id] = { stage: 'start', bytes: 0, total: 0 };
    renderSystem();
    setIndet('#cBar', true);
    try {
      var res = await invoke('cdrive_action', { id: id, confirm: true });
      var msg = res.freed > 0 ? t('disk.toast.cleanDone', { size: fmtSize(res.freed) }) : t('disk.toast.scanDone');
      if (res.failed) msg = t('disk.toast.failed', { err: res.failedItems[0] || '' });
      toast(msg, 5200);
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 6000);
    } finally {
      delete S.sysProgress[id];
      S.busy.system = false;
      setIndet('#cBar', false);
      if (id === 'winsxs') S.componentBytes = null;
      renderSystem();
      await loadSystem();
      try { S.disks = await invoke('cleanup_disks'); renderDisks(); renderC(); } catch (e2) { /* ignore */ }
    }
  }

  // ---------- 应用 ----------
  function renderApps() {
    var apps = S.apps;
    if (!apps.length) {
      $('#cApps').innerHTML = '<div class="empty">' + esc(t('disk.c.emptyApps')) + '</div>';
      $('#cAppsMeta').textContent = '';
      return;
    }
    if (!S.busy.apps) $('#cAppsMeta').textContent = t('disk.c.appList', { n: apps.length });
    var target = $('#cTargetDrive').value;
    var computing = !!S.busy.apps;
    var anyBusy = Object.keys(S.appBusy).length > 0;
    $('#cApps').innerHTML = apps.map(function (a) {
      var size = typeof a.size === 'number' ? a.size : (S.appSizes[a.path] || 0);
      var busyInfo = S.appBusy[a.path];
      var busy = !!busyInfo;
      var pct = busyInfo && busyInfo.total > 0
        ? Math.min(100, Math.round((busyInfo.bytes || 0) * 100 / busyInfo.total)) : null;
      var stageText = '';
      if (busy) {
        if (busyInfo.kind === 'uninstall') {
          stageText = busyInfo.stage === 'cleanup' ? t('disk.c.cleaningLeftover') : t('disk.c.uninstalling');
        } else {
          stageText = pct == null ? t('disk.c.migrating') : t('disk.c.migratingPct', { pct: pct });
        }
      }
      return '<div class="app-row" data-path="' + esc(a.path) + '">' +
        '<div class="app-main"><div class="app-name" title="' + esc(a.name) + '">' + esc(a.name) +
        (a.version ? ' <span class="meta">' + esc(a.version) + '</span>' : '') + '</div>' +
        '<div class="app-path" title="' + esc(a.path) + '">' + esc(a.path) + '</div>' +
        (busy ? '<div class="app-prog' + (pct == null ? ' indet' : '') + '">' +
          '<div class="progbar"><i style="width:' + (pct == null ? 35 : pct) + '%"></i></div>' +
          '<span>' + esc(stageText) + '</span></div>' : '') +
        '</div>' +
        '<div class="app-size">' + esc(computing ? '—' : fmtSize(size)) + '</div>' +
        '<div class="app-actions">' +
        '<button class="btn ghost app-uninstall" data-path="' + esc(a.path) + '"' +
        (busy || computing || anyBusy ? ' disabled' : '') + '>' +
        esc(busy && busyInfo.kind === 'uninstall' ? t('disk.c.uninstalling') : t('disk.c.uninstall')) + '</button>' +
        '<button class="btn app-migrate" data-path="' + esc(a.path) + '"' +
        (busy || computing || anyBusy || !target ? ' disabled' : '') + '>' +
        esc(busy && busyInfo.kind === 'migrate' ? t('disk.c.migrating') : t('disk.c.migrate')) + '</button>' +
        '</div></div>';
    }).join('');
    $('#cApps').querySelectorAll('.app-migrate').forEach(function (el) {
      el.addEventListener('click', function () { migrateApp(el.getAttribute('data-path')); });
    });
    $('#cApps').querySelectorAll('.app-uninstall').forEach(function (el) {
      el.addEventListener('click', function () { uninstallApp(el.getAttribute('data-path')); });
    });
  }

  async function loadApps() {
    if (S.busy.apps) return;
    S.busy.apps = true;
    $('#cAppsScan').disabled = true;
    $('#cApps').innerHTML = '<div class="empty">' + esc(t('disk.working')) + '</div>';
    try {
      S.apps = await invoke('apps_list');
      S.appSizes = {};
      S.appTotal = S.apps.length;
      if (S.apps.length) {
        renderApps();
        $('#cAppsMeta').textContent = t('disk.c.sizing', { n: '0', total: fmtNum(S.appTotal) });
        var list = await invoke('apps_sizes', { paths: S.apps.map(function (a) { return a.path; }) });
        (list || []).forEach(function (x) {
          S.appSizes[x.path] = x.size;
          S.apps.forEach(function (a) { if (a.path === x.path) a.size = x.size; });
        });
        S.apps.sort(function (a, b) { return (b.size || 0) - (a.size || 0); });
      }
      renderApps();
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 5200);
    } finally {
      S.busy.apps = false;
      $('#cAppsScan').disabled = false;
      renderApps();
      fillTargetDrives();
    }
  }

  async function migrateApp(path) {
    var app = S.apps.filter(function (a) { return a.path === path; })[0];
    if (!app) return;
    if (!S.admin) {
      toast(t('disk.toast.needAdmin'), 4200);
      return;
    }
    var target = $('#cTargetDrive').value;
    if (!target) { toast(t('disk.toast.noSelection')); return; }
    var name = app.name || path;
    var body = (isEn()
      ? 'Move "' + name + '" to ' + target + '\\AppMoved and leave a junction at the original path?\nQuit the app first. This moves ' + (S.appSizes[path] != null ? fmtSize(S.appSizes[path]) : 'the whole folder') + '.'
      : '将「' + name + '」移动到 ' + target + '\\AppMoved，并在原路径建立目录联接？\n请先退出该应用。将移动' + (S.appSizes[path] != null ? fmtSize(S.appSizes[path]) : '整个目录') + '。');
    if (!(await askConfirm(t('disk.confirmTitle'), body))) return;

    S.appBusy[path] = {
      kind: 'migrate',
      stage: 'start',
      bytes: 0,
      total: S.appSizes[path] || (app.size || 0),
    };
    renderApps();
    try {
      var res = await invoke('apps_migrate', { path: path, targetDrive: target, confirm: true });
      if (res.ok) {
        toast(t('disk.c.migrated', { target: res.target, size: fmtSize(res.bytes) }), 4600);
        S.apps = S.apps.filter(function (a) { return a.path !== path; });
        try { S.disks = await invoke('cleanup_disks'); } catch (e2) { /* ignore */ }
        renderDisks();
        fillTargetDrives();
        renderC();
      } else {
        toast(t('disk.toast.migrateFail', { note: res.note }), 6000);
      }
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 6000);
    } finally {
      delete S.appBusy[path];
      renderApps();
    }
  }

  async function uninstallApp(path) {
    var app = S.apps.filter(function (a) { return a.path === path; })[0];
    if (!app) return;
    var name = app.name || path;
    var body = (isEn()
      ? 'Run "' + name + '" own uninstaller, then delete leftover files in its install folder?\nQuit the app first. User data in AppData or documents is kept.'
      : '将启动「' + name + '」自带的卸载程序，完成后删除安装目录里的残留文件？\n请先退出该应用。AppData、文档等用户数据会保留。');
    if (!(await askConfirm(t('disk.c.uninstallTitle'), body))) return;

    S.appBusy[path] = { kind: 'uninstall', stage: 'uninstall', bytes: 0, total: 0 };
    renderApps();
    try {
      var res = await invoke('apps_uninstall', { path: path, confirm: true });
      if (res.ok) {
        if (res.freed > 0) toast(t('disk.c.uninstallDone', { size: fmtSize(res.freed) }), 4200);
        else toast(t('disk.c.uninstalled'), 4200);
        S.apps = S.apps.filter(function (a) { return a.path !== path; });
        try { S.disks = await invoke('cleanup_disks'); } catch (e2) { /* ignore */ }
        renderDisks();
        fillTargetDrives();
        renderC();
      } else {
        toast(t('disk.c.uninstallFail', { note: res.note }), 6000);
      }
    } catch (e) {
      toast(t('disk.toast.failed', { err: String(e) }), 6000);
    } finally {
      delete S.appBusy[path];
      renderApps();
    }
  }

  // ---------- 进度事件 ----------
  function listenProgress() {
    window.__TAURI__.event.listen('cleanup-progress', function (ev) {
      var p = ev.payload || {};
      var files = p.files || 0;
      var bytes = p.bytes || 0;
      if (p.task === 'junk') {
        if (p.stage) return;
        $('#junkMeta').textContent = t('disk.junk.scanning', { n: fmtNum(files) });
      } else if (p.task === 'large') {
        if (p.stage) return;
        $('#largeMeta').textContent = t('disk.progress.files', { n: fmtNum(files), size: fmtSize(bytes) });
      } else if (p.task === 'cdrive') {
        if (p.stage) {
          $('#cMeta').textContent = t('disk.c.analyzing', { path: S.cPath || 'C:\\' });
          return;
        }
        $('#cMeta').textContent = t('disk.progress.files', { n: fmtNum(files), size: fmtSize(bytes) });
      } else if (p.task === 'system') {
        if (p.stage) $('#cReclaim').textContent = t('disk.working');
      } else if (p.task === 'system-action') {
        if (!p.id) return;
        S.sysProgress[p.id] = {
          stage: p.stage || '',
          bytes: bytes,
          total: p.total || 0,
        };
        renderSystem();
      } else if (p.task === 'migrate') {
        var mi = S.appBusy[p.path];
        if (mi) {
          mi.stage = p.stage || '';
          mi.bytes = bytes;
          mi.total = p.total || mi.total || 0;
          renderApps();
        }
      } else if (p.task === 'uninstall') {
        var ui = S.appBusy[p.path];
        if (ui) {
          ui.stage = p.stage || '';
          ui.bytes = bytes;
          ui.total = p.total || ui.total || 0;
          renderApps();
        }
      } else if (p.task === 'apps') {
        if (p.stage) {
          $('#cAppsMeta').textContent = t('disk.c.sizing', { n: '0', total: fmtNum(S.appTotal || 0) });
          return;
        }
        $('#cAppsMeta').textContent = t('disk.c.sizing', { n: fmtNum(files), total: fmtNum(S.appTotal || 0) });
      } else if (p.task === 'cache') {
        if (p.stage) return;
        $('#cCacheMeta').textContent = t('disk.cache.progress', {
          n: fmtNum(files), size: fmtSize(bytes)
        });
      }
    });
    window.__TAURI__.event.listen('cleanup-tab', function (ev) {
      activateTab(typeof ev.payload === 'string' ? ev.payload : 'junk', true);
    });
  }

  // ---------- 绑定 ----------
  function bindUI() {
    $('#junkScan').addEventListener('click', scanJunk);
    $('#junkCancel').addEventListener('click', function () { invoke('cleanup_cancel').catch(function () {}); });
    $('#junkClean').addEventListener('click', cleanJunk);
    $('#junkAll').addEventListener('change', function () {
      var on = $('#junkAll').checked;
      var cats = S.junk ? S.junk.categories : [];
      S.selected = {};
      if (on) {
        cats.forEach(function (c) {
          if (!(c.needsAdmin && !S.admin)) S.selected[c.id] = true;
        });
      }
      renderJunk();
    });

    $('#largeScan').addEventListener('click', scanLarge);
    $('#largeCancel').addEventListener('click', function () { invoke('cleanup_cancel').catch(function () {}); });
    $('#largeDelete').addEventListener('click', deleteLarge);
    $('#largeMin').addEventListener('change', function () { if (S.large.length) scanLarge(); });
    $('#largeAll').addEventListener('change', function () {
      var on = $('#largeAll').checked;
      S.large.slice(0, LARGE_LIMIT).forEach(function (f) {
        if (on) S.largeSel[f.path] = true;
        else if (S.largeSel[f.path]) delete S.largeSel[f.path];
      });
      renderLarge();
    });

    $('#cAnalyze').addEventListener('click', function () {
      analyzeC(S.cPath || 'C:\\');
    });
    $('#cCancel').addEventListener('click', function () { invoke('cleanup_cancel').catch(function () {}); });
    $('#cUp').addEventListener('click', function () { if (S.cParent) analyzeC(S.cParent); });
    $('#cSystemScan').addEventListener('click', function () {
      S.componentBytes = null;
      loadSystem();
    });
    $('#cAppsScan').addEventListener('click', loadApps);
    $('#cTargetDrive').addEventListener('change', renderApps);
    $('#cCacheScan').addEventListener('click', scanCache);
    $('#cCacheClean').addEventListener('click', cleanCache);

    $('#elevateBtn').addEventListener('click', async function () {
      toast(t('disk.toast.elevating'), 4000);
      try { await invoke('cleanup_elevate'); } catch (e) {
        toast(t('disk.toast.failed', { err: String(e) }), 5200);
      }
    });
    $('#modalCancel').addEventListener('click', function () { closeModal(false); });
    $('#modalOk').addEventListener('click', function () { closeModal(true); });
    $('#modal').addEventListener('click', function (e) {
      if (e.target === $('#modal')) closeModal(false);
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && !$('#modal').classList.contains('hidden')) closeModal(false);
    });
  }

  async function boot() {
    var cfg = { lang: 'zh' };
    try { cfg = await invoke('config_get'); } catch (e) { /* keep zh */ }
    window.I18N.setLang((cfg && cfg.lang) || 'zh');
    window.I18N.applyStatic();

    try { S.admin = await invoke('cleanup_admin'); } catch (e) { S.admin = false; }
    $('#adminBadge').textContent = S.admin ? t('disk.admin.admin') : t('disk.admin.user');
    $('#adminBadge').classList.toggle('admin', S.admin);
    $('#elevateBtn').classList.toggle('hidden', S.admin);
    $('#topHint').textContent = S.admin ? '' : t('disk.elevateHint');

    try { S.disks = await invoke('cleanup_disks'); } catch (e) { S.disks = []; }
    S.disks.forEach(function (d) {
      if (d.letter.toUpperCase() !== 'C:') S.largeDrives[d.letter] = true;
    });
    if (!Object.keys(S.largeDrives).length && S.disks.length) {
      S.largeDrives[S.disks[0].letter] = true;
    }

    bindUI();
    renderDisks();
    fillTargetDrives();
    renderC();

    var tab = 'junk';
    try { tab = await invoke('cleanup_tab_get'); } catch (e) { /* junk */ }
    activateTab(tab, true);
    listenProgress();
  }

  window.addEventListener('DOMContentLoaded', boot);
})();
