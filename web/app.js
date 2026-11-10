'use strict';
const $ = s => document.querySelector(s);
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const DAYS = ['ma', 'di', 'wo', 'do', 'vr', 'za', 'zo'];
let state = {}, data = null, view = 'tasks', editing = null, timer = null;

async function api(path, method = 'GET', body) {
  const opt = { method, headers: {}, credentials: 'same-origin' };
  if (method !== 'GET') { opt.headers['X-AUSU'] = '1'; opt.headers['Content-Type'] = 'application/json'; opt.body = JSON.stringify(body || {}); }
  const r = await fetch('/api/' + path, opt);
  let j = {}; try { j = await r.json(); } catch {}
  if (r.status === 401 && path !== 'login') { await boot(); throw new Error(j.error || 'Log eerst in'); }
  if (!r.ok) throw new Error(j.error || `Fout ${r.status}`);
  return j;
}

function msg(text, ok = true) {
  const m = $('#msg'); m.textContent = text; m.className = 'msg ' + (ok ? 'ok' : 'err'); m.hidden = !text;
  clearTimeout(msg.t); if (text) msg.t = setTimeout(() => (m.hidden = true), ok ? 5000 : 12000);
}

function show(v) {
  view = v;
  for (const s of ['login', 'tasks', 'log', 'settings']) $('#v-' + s).hidden = s !== v;
  document.querySelectorAll('nav button').forEach(b => b.classList.toggle('active', b.dataset.view === v));
  if (v === 'log') loadLog();
  if (v === 'settings') renderSettings();
}

async function boot() {
  state = await (await fetch('/api/state')).json();
  if (!state.user) {
    $('#nav').hidden = true; $('#who').innerHTML = '';
    const setup = state.setup_needed;
    $('#loginTitle').textContent = setup ? 'Beheeraccount aanmaken' : 'Inloggen';
    $('#loginHint').textContent = setup
      ? (state.setup_allowed ? 'Er is nog geen account. Kies een gebruikersnaam en een wachtwoord van minimaal 10 tekens.' : 'Er is nog geen account. Maak het aan op de server zelf: open daar http://localhost:8470.')
      : '';
    $('#lPass2Row').hidden = !setup;
    $('#loginBtn').textContent = setup ? 'Account aanmaken' : 'Inloggen';
    $('#loginForm').hidden = setup && !state.setup_allowed;
    show('login');
    return;
  }
  $('#nav').hidden = false;
  $('#who').innerHTML = `${esc(state.user)} <button class="small" id="logout">Uitloggen</button>`;
  $('#logout').onclick = async () => { await api('logout', 'POST'); boot(); };
  await refresh();
  show(view === 'login' ? 'tasks' : view);
  clearInterval(timer); timer = setInterval(() => { if (!$('#taskDlg').open && view === 'tasks') refresh(); }, 5000);
}

$('#loginForm').onsubmit = async e => {
  e.preventDefault();
  const name = $('#lName').value.trim(), password = $('#lPass').value;
  try {
    if (state.setup_needed) {
      if (password !== $('#lPass2').value) return msg('De twee wachtwoorden zijn niet gelijk.', false);
      await api('setup', 'POST', { name, password });
    } else await api('login', 'POST', { name, password });
    $('#lPass').value = $('#lPass2').value = ''; msg(''); boot();
  } catch (err) { msg(err.message, false); }
};

document.querySelectorAll('nav button').forEach(b => (b.onclick = () => show(b.dataset.view)));

async function refresh() {
  try { data = await api('overview'); } catch (e) { return; }
  renderTasks();
  if (view === 'settings') renderSettings();
}

function pill(status) {
  const cls = /Fout/.test(status) ? 'err' : /Uploaden|Omzetten|wachtrij|Vrijgeven/.test(status) ? 'busy' : /Uit/.test(status) ? '' : 'ok';
  return `<span class="pill ${cls}">${esc(status)}</span>`;
}

function renderTasks() {
  const l = data.link;
  $('#linkBanner').innerHTML = l.needs_relink
    ? `<div class="banner">AudioShare heeft de koppeling ingetrokken. Koppel opnieuw onder <a href="#" data-go="settings">Instellingen</a>.</div>`
    : !l.linked ? `<div class="banner">Nog niet gekoppeld met AudioShare. Koppel eerst onder <a href="#" data-go="settings">Instellingen</a> met de code van een studio.</div>` : '';
  document.querySelectorAll('[data-go]').forEach(a => (a.onclick = e => { e.preventDefault(); show(a.dataset.go); }));
  $('#newTask').disabled = !l.linked;
  if (!data.tasks.length) { $('#taskList').innerHTML = `<div class="card empty">Nog geen taken. ${l.linked ? 'Klik op <strong>Taak toevoegen</strong>.' : ''}</div>`; return; }
  $('#taskList').innerHTML = data.tasks.map(x => {
    const t = x.task;
    const when = t.trigger === 'schedule'
      ? `${t.days.slice().sort().map(d => DAYS[d - 1]).join(' ')} om ${esc(t.time)}`
      : 'bewaken';
    return `<div class="card task">
      <div><h2>${esc(t.name)} ${pill(x.status)}</h2>
        <div class="meta">${esc(x.program || '(programma niet meer toegestaan)')} · ${esc(x.source_kind)} <code>${esc(t.source)}</code></div></div>
      <div class="acts">
        <button class="small" data-run="${t.id}" ${t.enabled && data.link.linked ? '' : 'disabled'}>Nu uploaden</button>
        <button class="small" data-edit="${t.id}">Wijzigen</button>
      </div>
      <div class="facts">
        <div><b>Moment</b>${when}</div>
        ${x.next ? `<div><b>Volgende</b>${esc(x.next)}</div>` : ''}
        <div><b>Laatste upload</b>${x.last_upload_at ? `${esc(x.last_upload_at)} — ${esc(x.last_upload_title)}` : '—'}</div>
        ${x.job_message ? `<div><b>Bezig</b>${esc(x.job_message)}</div>` : ''}
      </div>
      ${x.status === 'Fout' && x.last_error ? `<div class="err-line">${esc(x.last_error_at)}: ${esc(x.last_error)}</div>` : ''}
    </div>`;
  }).join('');
  document.querySelectorAll('[data-run]').forEach(b => (b.onclick = async () => {
    try { await api(`tasks/${b.dataset.run}/run`, 'POST'); msg('Upload gestart. De status staat bij de taak.'); setTimeout(refresh, 1500); }
    catch (e) { msg(e.message, false); }
  }));
  document.querySelectorAll('[data-edit]').forEach(b => (b.onclick = () => openTask(data.tasks.find(x => x.task.id === b.dataset.edit).task)));
}

// ---------- Task dialog ----------
function dayBoxes(el, chosen) {
  el.innerHTML = DAYS.map((d, i) => `<label><input type="checkbox" value="${i + 1}" ${chosen.includes(i + 1) ? 'checked' : ''}><span>${d}</span></label>`).join('');
  el.querySelectorAll('input').forEach(i => (i.onchange = preview));
}
const checkedDays = el => [...el.querySelectorAll('input:checked')].map(i => +i.value);
const radio = n => document.querySelector(`input[name=${n}]:checked`).value;

function openTask(t) {
  editing = t || null;
  t = t || { name: '', enabled: true, program_id: data.programs[0]?.id || 0, source_kind: 'file', source: '', trigger: 'watch', days: [], time: '16:00', settle_secs: 60, date_rule: 'today', air_days: [], title_template: '{programma} {datum}', description: '' };
  $('#taskDlgTitle').textContent = editing ? 'Taak wijzigen' : 'Taak toevoegen';
  $('#tName').value = t.name;
  $('#tProgram').innerHTML = data.programs.map(p => `<option value="${p.id}" ${p.id === t.program_id ? 'selected' : ''}>${esc(p.name)}</option>`).join('');
  $('#tEnabled').checked = t.enabled;
  document.querySelector(`input[name=tKind][value=${t.source_kind}]`).checked = true;
  $('#tSource').value = t.source;
  document.querySelector(`input[name=tTrigger][value=${t.trigger}]`).checked = true;
  dayBoxes($('#tDays'), t.days); dayBoxes($('#tAirDays'), t.air_days);
  $('#tTime').value = t.time; $('#tSettle').value = t.settle_secs;
  $('#tDateRule').value = t.date_rule; $('#tTitle').value = t.title_template; $('#tDesc').value = t.description;
  $('#tDelete').hidden = !editing;
  toggles(); preview();
  $('#taskDlg').showModal();
}

function collect() {
  return {
    id: editing?.id || '', name: $('#tName').value, enabled: $('#tEnabled').checked, program_id: +$('#tProgram').value,
    source_kind: radio('tKind'), source: $('#tSource').value, trigger: radio('tTrigger'),
    days: checkedDays($('#tDays')), time: $('#tTime').value, settle_secs: +$('#tSettle').value || 60,
    date_rule: $('#tDateRule').value, air_days: checkedDays($('#tAirDays')),
    title_template: $('#tTitle').value, description: $('#tDesc').value,
  };
}

function toggles() {
  $('#schedRow').hidden = radio('tTrigger') !== 'schedule';
  $('#airRow').hidden = $('#tDateRule').value !== 'next_air_day';
}

let pvTimer;
function preview() {
  toggles();
  clearTimeout(pvTimer);
  pvTimer = setTimeout(async () => {
    try {
      const p = await api('preview', 'POST', collect());
      const lines = [];
      if (p.file_error) lines.push(`<div class="bad">${esc(p.file_error)}</div>`);
      if (p.file) lines.push(`Bestand nu: <strong>${esc(p.file)}</strong> (${p.size_mb ?? '?'} MB, ${esc(p.modified || '')})`);
      if (p.date) lines.push(`Uitzenddatum: <strong>${esc(p.date)}</strong>`);
      if (p.date_error) lines.push(`<div class="bad">${esc(p.date_error)}</div>`);
      if (p.title) lines.push(`Titel: <strong>${esc(p.title)}</strong>`);
      if (p.next) lines.push(`Volgende geplande upload: <strong>${esc(p.next)}</strong>`);
      if (!editing && radio('tTrigger') === 'watch') lines.push('<span class="muted">Het bestand dat er nu staat, geldt als al verstuurd. Wil je dat ook uploaden, gebruik dan <strong>Nu uploaden</strong>.</span>');
      $('#preview').innerHTML = lines.join('<br>');
    } catch (e) { $('#preview').textContent = e.message; }
  }, 300);
}
['#tName', '#tSource', '#tTitle', '#tTime', '#tSettle'].forEach(s => $(s).addEventListener('input', preview));
['#tProgram', '#tDateRule'].forEach(s => $(s).addEventListener('change', preview));
document.querySelectorAll('input[name=tKind], input[name=tTrigger]').forEach(i => i.addEventListener('change', preview));

$('#newTask').onclick = () => openTask(null);
$('#tCancel').onclick = () => $('#taskDlg').close();
$('#tSave').onclick = async () => {
  try { await api('tasks', 'POST', collect()); $('#taskDlg').close(); msg('Taak opgeslagen.'); refresh(); }
  catch (e) { $('#preview').innerHTML = `<div class="bad">${esc(e.message)}</div>`; }
};
$('#tDelete').onclick = async () => {
  if (!editing || !confirm(`Taak "${editing.name}" verwijderen?`)) return;
  try { await api(`tasks/${editing.id}`, 'DELETE'); $('#taskDlg').close(); msg('Taak verwijderd.'); refresh(); }
  catch (e) { msg(e.message, false); }
};

// ---------- Log ----------
async function loadLog() {
  try {
    const { log } = await api('log');
    $('#logBody').innerHTML = log.length
      ? log.map(l => `<tr class="${esc(l.level)}"><td>${esc(l.time)}</td><td>${esc(l.who)}</td><td>${esc(l.text)}</td></tr>`).join('')
      : '<tr><td colspan="3" class="muted">Nog niets gebeurd.</td></tr>';
  } catch (e) { msg(e.message, false); }
}
$('#reloadLog').onclick = loadLog;

// ---------- Settings ----------
function renderSettings() {
  if (!data) return;
  const l = data.link;
  $('#linkInfo').innerHTML = l.linked
    ? `<p>Gekoppeld als studio <strong>${esc(l.studio)}</strong> met ${esc(l.url)} (apparaat ${esc(l.device)}, sinds ${esc(l.linked_at)}).<br>
       <span class="muted">Mag aanleveren voor: ${data.programs.map(p => esc(p.name)).join(', ') || '— (vink programma\'s aan in AudioShare)'}</span></p>`
    : l.needs_relink ? '<p class="bad">De koppeling is ingetrokken. Koppel opnieuw met een nieuwe code.</p>' : '<p class="muted">Nog niet gekoppeld.</p>';
  $('#linkForm').hidden = l.linked;
  $('#linkActions').hidden = !l.linked;
  if (!$('#kDevice').value) $('#kDevice').value = data.computer;
  if (!$('#kUrl').value && l.url) $('#kUrl').value = l.url;
  $('#sPort').value = data.settings.port; $('#sRemote').checked = data.settings.allow_remote;
  $('#accounts').innerHTML = data.settings.accounts.map(a => `<div class="acc"><span>${esc(a)}${a === state.user ? ' <span class="muted">(jij)</span>' : ''}</span>
    ${a === state.user ? '' : `<button class="small danger" data-delacc="${esc(a)}">Verwijderen</button>`}</div>`).join('');
  document.querySelectorAll('[data-delacc]').forEach(b => (b.onclick = async () => {
    if (!confirm(`Account ${b.dataset.delacc} verwijderen?`)) return;
    try { await api('accounts/' + encodeURIComponent(b.dataset.delacc), 'DELETE'); refresh(); } catch (e) { msg(e.message, false); }
  }));
  $('#shareList').innerHTML = (data.shares || []).map(x => `<div class="acc"><span><strong>${esc(x.letter || '—')}</strong> ${esc(x.unc)} <span class="muted">als ${esc(x.user || '(service-account)')}</span></span>
    <span><button class="small" data-shtest="${esc(x.unc)}">Testen</button> <button class="small" data-shedit="${esc(x.unc)}">Wijzigen</button> <button class="small danger" data-shdel="${esc(x.unc)}">Verwijderen</button></span></div>`).join('');
  document.querySelectorAll('[data-shtest]').forEach(b => (b.onclick = async () => {
    try { msg((await api('shares/test', 'POST', { unc: b.dataset.shtest })).message); } catch (e) { msg(e.message, false); }
  }));
  document.querySelectorAll('[data-shedit]').forEach(b => (b.onclick = () => {
    const x = data.shares.find(s => s.unc === b.dataset.shedit);
    $('#shLetter').value = (x.letter || '').replace(':', ''); $('#shUnc').value = x.unc; $('#shUser').value = x.user; $('#shPass').value = '';
    $('#shPass').focus();
  }));
  document.querySelectorAll('[data-shdel]').forEach(b => (b.onclick = async () => {
    if (!confirm(`Netwerkmap ${b.dataset.shdel} verwijderen?`)) return;
    try { await api('shares/delete', 'POST', { unc: b.dataset.shdel }); refresh(); } catch (e) { msg(e.message, false); }
  }));
  $('#version').textContent = `AudioShare Uploader ${data.version}`;
}

$('#linkForm').onsubmit = async e => {
  e.preventDefault();
  const f = $('#kFile').files[0];
  const body = { url: $('#kUrl').value, code: $('#kCode').value, device_name: $('#kDevice').value };
  if (f) body.file = await f.text();
  try { const r = await api('link', 'POST', body); msg(r.message); $('#kCode').value = ''; $('#kFile').value = ''; refresh(); }
  catch (err) { msg(err.message, false); }
};
$('#testBtn').onclick = async () => { try { msg((await api('test', 'POST')).message); refresh(); } catch (e) { msg(e.message, false); } };
$('#unlinkBtn').onclick = async () => {
  if (!confirm('Koppeling met AudioShare verwijderen? Trek de sleutel ook in bij AudioShare → Recorders → Studio\'s.')) return;
  await api('unlink', 'POST'); refresh();
};
$('#shareForm').onsubmit = async e => {
  e.preventDefault();
  try {
    const r = await api('shares', 'POST', { letter: $('#shLetter').value, unc: $('#shUnc').value, user: $('#shUser').value, password: $('#shPass').value });
    msg(r.message, r.ok); if (r.ok) { $('#shLetter').value = $('#shUnc').value = $('#shUser').value = $('#shPass').value = ''; }
    refresh();
  } catch (err) { msg(err.message, false); }
};
$('#webForm').onsubmit = async e => {
  e.preventDefault();
  try { msg((await api('settings', 'POST', { port: +$('#sPort').value, allow_remote: $('#sRemote').checked })).message); refresh(); }
  catch (err) { msg(err.message, false); }
};
$('#pwForm').onsubmit = async e => {
  e.preventDefault();
  try { await api('password', 'POST', { old: $('#pOld').value, new: $('#pNew').value }); $('#pOld').value = $('#pNew').value = ''; msg('Wachtwoord gewijzigd.'); }
  catch (err) { msg(err.message, false); }
};
$('#accForm').onsubmit = async e => {
  e.preventDefault();
  try { await api('accounts', 'POST', { name: $('#aName').value, password: $('#aPass').value }); $('#aName').value = $('#aPass').value = ''; msg('Account toegevoegd.'); refresh(); }
  catch (err) { msg(err.message, false); }
};

boot();
