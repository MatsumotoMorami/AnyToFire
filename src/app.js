let items = [];
let recentItems = [];
const selection = new Set();
const recentSelection = new Set();
let activeModal = null;
let settingsReceived = false;
let showSettingsAfterGuide = false;
let initPendingVersion = null;
let initializing = false;
let initialSetup = false;
let initAction = 'init_prepare';
let initSourcePending = false;
let lastHookSource = 'cfg';
const initStep = id => {
  ['version','score','migrate','inject'].forEach(name => el('init-step-' + name).classList.toggle('active', name === id));
};
function runInit(action) { initAction = action; el('init-retry').hidden = true; call(action); }
function startInitialization(version) {
  initializing = true;
  initialSetup = true;
  el('init-backdrop').classList.add('open');
  el('init-version').textContent = version === 33 ? 'Sparkle Shower' : version === 34 ? 'ZINRAI' : '未知版本';
  el('init-version').className = 'init-version v' + version;
  el('init-choice').hidden = true;
  el('init-steps').hidden = false;
  el('init-step-version').classList.add('done');
  initStep('score');
  el('init-status').textContent = '正在准备本地分数数据库与旧曲库索引…';
  runInit('init_prepare');
}
function finishInitialization() {
  const start = performance.now();
  const tick = () => {
    const remaining = Math.max(0, 3000 - (performance.now() - start));
    if (remaining > 0) {
      el('init-status').textContent = `初始化完成，${Math.ceil(remaining / 1000)} 秒后进入设置`;
      setTimeout(tick, Math.min(200, remaining));
    } else {
      initializing = false;
      el('init-backdrop').classList.remove('open');
      el('settings-backdrop').classList.add('open','initial-setup');
      el('access-code').focus();
    }
  };
  tick();
}
const el = id => document.getElementById(id);
const icon = name => `<svg fill="none" stroke="currentColor"><use href="#i-${name}"/></svg>`;
const escapeHtml = value => String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch]));
function displayTime(value) {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value ?? '').replace('T', ' ').replace(/\.\d+/, '').slice(0, 19);
  const pad = number => String(number).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}
const call = (action, data = {}) => window.ipc.postMessage(JSON.stringify({action, ...data}));
const iconClass = item => item.source === 'BJM' ? 'bjm' : item.source === 'Firenet' ? 'fire' : item.source === 'Fervidex' ? 'fer' : item.source === '合并' ? 'merged' : 'other';
const iconText = item => item.source === 'BJM' ? 'BJ' : item.source === 'Firenet' ? 'FN' : item.source === 'Fervidex' ? 'FV' : item.source === '合并' ? 'M' : 'EA';

function filtered() {
  const query = el('search').value.trim().toLocaleLowerCase();
  const source = el('source-filter').value;
  const order = el('sort').value;
  return items.filter(x => (source === 'all' || x.source === source) && (!query || [x.name,x.player,x.iidx,x.url].some(v => String(v).toLocaleLowerCase().includes(query))))
    .sort((a,b) => order === 'charts' ? b.charts-a.charts : order === 'oldest' ? a.date.localeCompare(b.date) : b.date.localeCompare(a.date));
}
function render() {
  const sources = [...new Set(items.map(x => x.source))].sort();
  const filter = el('source-filter');
  const current = filter.value;
  filter.innerHTML = '<option value="all">全部来源</option>' + sources.map(x => `<option value="${escapeHtml(x)}">${escapeHtml(x)}</option>`).join('');
  filter.value = sources.includes(current) ? current : 'all';
  const visible = filtered();
  el('rows').innerHTML = visible.map(item => `<tr class="${selection.has(item.id) ? 'selected' : ''}">
    <td><input type="checkbox" class="row-check" data-id="${escapeHtml(item.id)}" aria-label="选择 ${escapeHtml(item.name)}" ${selection.has(item.id) ? 'checked' : ''}></td>
    <td><div class="source"><div class="source-icon ${iconClass(item)}">${item.source === 'Firenet' ? '<img src="__FIRE_ICON__" alt="">' : escapeHtml(iconText(item))}</div><div><div class="source-name">${escapeHtml(item.name)}</div><div class="source-sub" title="${escapeHtml(item.url)}">${escapeHtml(item.url || item.source)}</div></div></div></td>
    <td><div class="player-name">${escapeHtml(item.player)}</div><div class="mono">${escapeHtml(item.iidx)}</div></td>
    <td><div class="count">${item.charts.toLocaleString()}</div><div class="count-sub">SP ${item.sp.toLocaleString()} · DP ${item.dp.toLocaleString()}</div></td>
    <td><span class="badge ${item.kind === '合并结果' ? 'merged' : ''}">${escapeHtml(item.kind)}</span></td>
    <td class="mono">${escapeHtml(item.date)}</td>
    <td><div class="row-actions"><button class="icon-btn row-export" data-id="${escapeHtml(item.id)}" title="导出">${icon('download')}</button><button class="icon-btn row-rename" data-id="${escapeHtml(item.id)}" title="重命名">${icon('edit')}</button><button class="icon-btn row-identity" data-id="${escapeHtml(item.id)}" title="设置 IIDX ID">${icon('info')}</button><button class="icon-btn row-delete" data-id="${escapeHtml(item.id)}" title="删除档案" aria-label="删除 ${escapeHtml(item.name)}">${icon('trash')}</button></div></td>
  </tr>`).join('');
  el('empty').classList.toggle('show', visible.length === 0);
  el('empty').textContent = items.length ? '无结果' : '暂无成绩';
  el('visible-count').textContent = `显示 ${visible.length} 份档案`;
  const selected = selection.size;
  el('selection-status').textContent = selected ? `已选择 ${selected} 份` : '尚未选择';
  el('hero-selected').textContent = selected;
  el('hero-merge').disabled = selected < 2;
  el('export-selected').disabled = selected !== 1;
  el('select-all').checked = visible.length > 0 && visible.every(x => selection.has(x.id));
  el('select-all').indeterminate = visible.some(x => selection.has(x.id)) && !el('select-all').checked;
  el('stat-sets').textContent = items.length.toLocaleString();
  el('stat-charts').textContent = items.reduce((n,x) => n+x.charts,0).toLocaleString();
  el('stat-servers').textContent = new Set(items.filter(x => x.source !== '合并').map(x => x.source)).size;
}
function toast(message) {
  el('toast').textContent = message;
  el('toast').classList.add('show');
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => el('toast').classList.remove('show'), 4200);
}
window.anytofireReceive = message => {
  if (message.type === 'merge_complete') {
    selection.clear();
  } else if (message.type === 'items') {
    items = message.items;
    for (const id of selection) if (!items.some(x => x.id === id)) selection.delete(id);
    render();
  } else if (message.type === 'settings') {
    el('plugin-enabled').setAttribute('aria-checked', String(message.pluginEnabled));
    lastHookSource = message.hookSource || 'cfg';
    el('hook-source').value = lastHookSource;
    el('init-hook-source').value = lastHookSource;
    el('plugin-enabled').disabled = false;
    el('auto-sync').setAttribute('aria-checked', String(message.autoSync));
    el('omni-overlay').setAttribute('aria-checked', String(message.omniOverlay));
    el('access-code').value = '';
    el('access-code').placeholder = message.hasAccessCode ? '已填写' : '填写访问码';
    if (initialSetup && !initializing && message.hasAccessCode) { initialSetup = false; el('settings-backdrop').classList.remove('initial-setup'); }
    settingsReceived = true;
  } else if (message.type === 'hook_state') {
    el('plugin-enabled').setAttribute('aria-checked', String(message.pluginEnabled));
    lastHookSource = message.hookSource || 'cfg';
    el('hook-source').value = lastHookSource;
    el('init-hook-source').value = lastHookSource;
    el('plugin-enabled').disabled = false;
  } else if (message.type === 'initialization' && message.needed) {
    initPendingVersion = message.version;
    openGuide();
  } else if (message.type === 'hook_source_saved') {
    if (initSourcePending) {
      initSourcePending = false;
      const version = initPendingVersion;
      initPendingVersion = null;
      startInitialization(version);
    }
  } else if (message.type === 'initialization' && !message.hasAccessCode) {
    showSettingsAfterGuide = true;
    initialSetup = true;
    openGuide();
  } else if (message.type === 'init_prepared') {
    el('init-step-score').classList.add('done');
    if (message.migrationNeeded) {
      el('init-step-migrate').hidden = false;
      initStep('migrate');
      el('init-status').textContent = '正在迁移旧版本 IIDXScore 成绩…';
      runInit('init_migrate');
    } else {
      initStep('inject');
      el('init-status').textContent = '正在更新 DLL 注入配置…';
      runInit('init_inject');
    }
  } else if (message.type === 'init_migrated') {
    el('init-step-migrate').classList.add('done');
    initStep('inject');
    el('init-status').textContent = '正在更新 DLL 注入配置…';
    runInit('init_inject');
  } else if (message.type === 'init_complete') {
    el('init-step-inject').classList.add('done');
    initStep('');
    finishInitialization();
  } else if (message.type === 'recent') {
    recentItems = message.items;
    for (const id of recentSelection) if (!recentItems.some(x => x.eventId === id)) recentSelection.delete(id);
    renderRecent();
    if (el('refresh-recent').disabled) {
      el('refresh-recent').disabled = false;
      el('recent-list').classList.remove('list-refreshed');
      void el('recent-list').offsetWidth;
      el('recent-list').classList.add('list-refreshed');
      toast('最近游玩分数已刷新');
    }
  } else if (message.type === 'error' && initializing) {
    el('init-status').textContent = message.message;
    el('init-retry').hidden = false;
  } else if (message.type === 'toast' || message.type === 'error') {
    if (message.type === 'error') el('refresh-recent').disabled = false;
    if (message.type === 'error') { el('hook-source').value = lastHookSource; el('init-start').disabled = false; el('plugin-enabled').disabled = false; initSourcePending = false; }
    toast(message.message);
  }
};
function renderRecent() {
  const list = el('recent-list');
  list.classList.toggle('recent-empty', recentItems.length === 0);
  list.innerHTML = recentItems.length ? `<table><thead><tr><th><input id="recent-select-all" type="checkbox" aria-label="选择所有可上传单局"></th><th>时间</th><th>游戏版本</th><th>曲目</th><th>EX Score</th><th>同步</th></tr></thead><tbody>${recentItems.map(x => `<tr class="${x.uploadedAt ? 'recent-uploaded' : ''} ${recentSelection.has(x.eventId) ? 'selected' : ''}"><td><input class="recent-check" type="checkbox" data-id="${escapeHtml(x.eventId)}" aria-label="选择 ${escapeHtml(x.title)}" ${recentSelection.has(x.eventId) ? 'checked' : ''} ${!x.uploadAllowed ? 'disabled' : ''}></td><td>${escapeHtml(displayTime(x.occurredAt))}</td><td>${versionName(x.module)}</td><td class="recent-title" title="${escapeHtml(x.title)}">${escapeHtml(x.title)}</td><td>${x.exScore == null ? '—' : escapeHtml(x.exScore)}</td><td>${syncBadge(x)}</td></tr>`).join('')}</tbody></table>` : '暂无最近游玩分数';
  const all = recentItems.filter(x => x.uploadAllowed);
  if (all.length) el('recent-select-all').checked = all.every(x => recentSelection.has(x.eventId));
  el('upload-recent').disabled = recentSelection.size === 0;
  el('upload-recent').textContent = `上传所选 ${recentSelection.size}`;
  el('select-pending-recent').disabled = !recentItems.some(x => x.uploadAllowed && !x.uploadedAt);
  el('select-all-recent').disabled = !recentItems.some(x => x.uploadAllowed);
  el('select-all-recent').textContent = recentSelection.size ? '取消选中' : '全选';
}
function versionName(module) { return module === 'IIDX33music' ? 'Sparkle Shower' : module === 'IIDX34music' ? 'ZINRAI' : '未知'; }
function syncBadge(item) {
  let kind, label, detail;
  if (item.uploadedAt) { kind = 'uploaded'; label = '已上传'; detail = `成功 ${Math.max(1, item.uploadCount || 0)} 次`; }
  else if (!item.uploadAllowed || /HTTP 401|ACCESS_CODE_INVALID|ACCESS_CODE_EXPIRED|访问码.{0,8}拒绝/.test(item.blocked || '')) { kind = 'refused'; label = '拒绝'; detail = item.blocked || '曲目或谱面不在受支持的 AC 曲库（omni、删除曲等）'; }
  else if (item.blocked) { kind = 'error'; label = '错误'; detail = item.blocked; }
  else { kind = 'pending'; label = '待上传'; detail = ''; }
  return `<span class="sync-badge ${kind}"${detail ? ` title="${escapeHtml(detail)}"` : ''}>${label}</span>`;
}
function openModal(config) {
  activeModal = config;
  el('modal-title').textContent = config.title;
  el('modal-label').textContent = config.label || '';
  el('modal-input').value = config.value || '';
  el('modal-input').style.display = config.input === false ? 'none' : '';
  el('modal-label').style.display = config.label ? '' : 'none';
  el('modal-format').style.display = config.type === 'export' ? '' : 'none';
  el('modal-confirm').textContent = config.action;
  el('modal-backdrop').classList.add('open');
  if (config.input !== false) setTimeout(() => { el('modal-input').focus(); el('modal-input').select(); }, 30);
}
function closeModal() { el('modal-backdrop').classList.remove('open'); activeModal = null; }
function mergeDialog() {
  const chosen = items.filter(x => selection.has(x.id));
  if (chosen.length < 2) return;
  if (chosen.some(x => x.iidx === '—')) { toast('请先设置所有成绩的 IIDX ID'); return; }
  if (new Set(chosen.map(x => x.iidx)).size !== 1) { toast('IIDX ID 不同，无法合并'); return; }
  openModal({type:'merge', title:`合并 ${chosen.length} 份成绩`, label:'名称', value:`合并成绩 · ${new Date().toISOString().slice(0,10)}`, action:'合并'});
}
function renameDialog(id) { const item = items.find(x => x.id === id); if (item) openModal({type:'rename', id, title:'重命名成绩', label:'名称', value:item.name, action:'保存'}); }
function deleteDialog(id) { const item = items.find(x => x.id === id); if (item) openModal({type:'delete', id, title:`删除 · ${item.name}`, label:'将从列表移除，档案文件保留在 AnyToFire_exports/.deleted，可手动恢复。', input:false, action:'删除档案'}); }
function identityDialog(id) { const item = items.find(x => x.id === id); if (item) openModal({type:'identity', id, title:'设置 IIDX ID', label:'IIDX ID', value:item.iidx === '—' ? '' : item.iidx, action:'保存'}); }
function exportDialog(id) { const item = items.find(x => x.id === id); if (item) openModal({type:'export', id, title:`导出 · ${item.name}`, input:false, action:'导出'}); }
function uploadRecentDialog() {
  const chosen = recentItems.filter(x => recentSelection.has(x.eventId));
  if (!chosen.length) return;
  const duplicates = chosen.filter(x => x.uploadedAt).length;
  if (duplicates) openModal({type:'reupload', title:'确认再次上传？', label:`所选 ${chosen.length} 项中有 ${duplicates} 项已上传。是否再次向火网发送这些成绩？`, input:false, action:'仍要上传'});
  else call('upload_recent', {ids:chosen.map(x => x.eventId), confirmDuplicate:false});
}
function confirmModal() {
  if (!activeModal) return;
  const config = activeModal, value = el('modal-input').value.trim();
  if (config.type === 'merge') {
    if (!value) { toast('请填写名称'); return; }
    call('merge', {ids:[...selection], name:value});
  } else if (config.type === 'rename') {
    if (!value) { toast('名称不能为空'); return; }
    call('rename', {id:config.id, name:value});
  } else if (config.type === 'identity') call('identity', {id:config.id, iidx:value});
  else if (config.type === 'delete') call('delete', {id:config.id});
  else if (config.type === 'reupload') call('upload_recent', {ids:[...recentSelection], confirmDuplicate:true});
  else if (config.type === 'export') call('export', {id:config.id, format:({ 'Fervidex JSON':'json','KONAMI CSV':'csv','Asphyxia DB':'db' })[el('modal-format').value]});
  closeModal();
}
el('rows').addEventListener('change', event => { if (!event.target.matches('.row-check')) return; event.target.checked ? selection.add(event.target.dataset.id) : selection.delete(event.target.dataset.id); render(); });
el('recent-list').addEventListener('change', event => {
  if (event.target.id === 'recent-select-all') recentItems.filter(x => x.uploadAllowed).forEach(x => event.target.checked ? recentSelection.add(x.eventId) : recentSelection.delete(x.eventId));
  else if (event.target.matches('.recent-check')) event.target.checked ? recentSelection.add(event.target.dataset.id) : recentSelection.delete(event.target.dataset.id);
  renderRecent();
});
el('select-pending-recent').addEventListener('click', () => { recentSelection.clear(); recentItems.filter(x => x.uploadAllowed && !x.uploadedAt).forEach(x => recentSelection.add(x.eventId)); renderRecent(); });
el('select-all-recent').addEventListener('click', () => { if (recentSelection.size) recentSelection.clear(); else recentItems.filter(x => x.uploadAllowed).forEach(x => recentSelection.add(x.eventId)); renderRecent(); });
el('upload-recent').addEventListener('click', uploadRecentDialog);
el('refresh-recent').addEventListener('click', () => { el('refresh-recent').disabled = true; call('refresh'); });
el('rows').addEventListener('click', event => {
  const exportButton = event.target.closest('.row-export');
  const renameButton = event.target.closest('.row-rename');
  const identityButton = event.target.closest('.row-identity');
  const deleteButton = event.target.closest('.row-delete');
  if (exportButton) exportDialog(exportButton.dataset.id);
  if (renameButton) renameDialog(renameButton.dataset.id);
  if (identityButton) identityDialog(identityButton.dataset.id);
  if (deleteButton) deleteDialog(deleteButton.dataset.id);
});
el('select-all').addEventListener('change', event => { filtered().forEach(x => event.target.checked ? selection.add(x.id) : selection.delete(x.id)); render(); });
['search','source-filter','sort'].forEach(id => el(id).addEventListener(id === 'search' ? 'input' : 'change', render));
el('hero-merge').addEventListener('click', mergeDialog);
function switchView(view) {
  const archives = view === 'archives';
  el('tab-archives').classList.toggle('active', archives);
  el('tab-recent').classList.toggle('active', !archives);
  el('tab-archives').setAttribute('aria-selected', String(archives));
  el('tab-recent').setAttribute('aria-selected', String(!archives));
  el('archive-view').hidden = !archives;
  el('recent-view').hidden = archives;
  el('archive-actions').hidden = !archives;
}
el('tab-archives').addEventListener('click', () => switchView('archives'));
el('tab-recent').addEventListener('click', () => switchView('recent'));
function closeSettings() { if (initialSetup) return; el('settings-backdrop').classList.remove('open'); el('settings-open').focus(); }
el('settings-open').addEventListener('click', () => { el('settings-backdrop').classList.add('open'); el('auto-sync').focus(); });
el('settings-close').addEventListener('click', closeSettings);
el('settings-backdrop').addEventListener('click', event => { if (event.target === el('settings-backdrop')) closeSettings(); });
el('init-retry').addEventListener('click', () => runInit(initAction));
el('auto-sync').addEventListener('click', () => { const enabled = el('auto-sync').getAttribute('aria-checked') !== 'true'; el('auto-sync').setAttribute('aria-checked', String(enabled)); });
el('plugin-enabled').addEventListener('click', () => { const enabled = el('plugin-enabled').getAttribute('aria-checked') !== 'true'; el('plugin-enabled').disabled = true; call('set_plugin_enabled', {enabled}); });
el('hook-source').addEventListener('change', () => call('set_hook_source', {source: el('hook-source').value}));
el('init-start').addEventListener('click', () => { initSourcePending = true; el('init-start').disabled = true; call('set_hook_source', {source: el('init-hook-source').value}); });
el('omni-overlay').addEventListener('click', () => { const enabled = el('omni-overlay').getAttribute('aria-checked') !== 'true'; el('omni-overlay').setAttribute('aria-checked', String(enabled)); });
el('save-sync').addEventListener('click', () => call('set_sync', {autoSync: el('auto-sync').getAttribute('aria-checked') === 'true', omniOverlay: el('omni-overlay').getAttribute('aria-checked') === 'true', accessCode: el('access-code').value}));
el('logout').addEventListener('click', () => call('logout'));
function openGuide() { el('guide-backdrop').classList.add('open'); el('guide-button').setAttribute('aria-expanded', 'true'); el('guide-close').focus(); }
function closeGuide() {
  if (!el('guide-backdrop').classList.contains('open')) return;
  el('guide-backdrop').classList.remove('open'); el('guide-button').setAttribute('aria-expanded', 'false');
  if (initPendingVersion !== null) { el('init-backdrop').classList.add('open'); el('init-choice').hidden = false; el('init-steps').hidden = true; el('init-status').textContent = ''; }
  else if (showSettingsAfterGuide) { showSettingsAfterGuide = false; el('settings-backdrop').classList.add('open','initial-setup'); el('access-code').focus(); }
  else el('guide-button').focus();
}
el('guide-button').addEventListener('click', openGuide);
el('guide-close').addEventListener('click', closeGuide);
el('guide-backdrop').addEventListener('click', event => { if (event.target === el('guide-backdrop')) closeGuide(); });
el('export-selected').addEventListener('click', () => { const id = [...selection][0]; if (id) exportDialog(id); });
el('refresh').addEventListener('click', () => call('refresh'));
el('import-demo').addEventListener('click', () => call('import'));
el('modal-cancel').addEventListener('click', closeModal);
el('modal-confirm').addEventListener('click', confirmModal);
el('modal-backdrop').addEventListener('click', event => { if (event.target === el('modal-backdrop')) closeModal(); });
document.addEventListener('keydown', event => { if (event.key === 'Escape') { if (el('settings-backdrop').classList.contains('open')) closeSettings(); closeModal(); closeGuide(); } if (event.key === 'Enter' && activeModal) confirmModal(); });
render();
switchView('recent');
call('ready');
