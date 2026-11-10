<script lang="ts">
  import {
    get,
    message,
    send,
    runSyncDryRun,
    runSync,
    getSyncStatus,
    getSyncHistory,
    getSyncSchedule,
    saveSyncSchedule,
    getUnresolvedDiagnostics,
    setManualSyncMapping,
    resetSyncMapping,
    getAcquisitionSettings,
    putAcquisitionSettings,
    testTorBoxConnection,
    getAcquisitionProfiles,
    createAcquisitionProfile,
    updateAcquisitionProfile,
    deleteAcquisitionProfile,
    duplicateAcquisitionProfile,
    getSourceHealthReports,
    runLibraryDryRun,
    executeBatchNormalize,
    retryNormalizationRefresh,
    getNormalizationHistory,
    rollbackNormalizationRecord,
    type LibraryAnalysisSummary,
    type NormalizationHistoryRecord,
    type NormalizeItemRequest,
    type AcquisitionProfile,
    type SourceHealthReport,
    type AcquisitionConfig,
    type AcquisitionConfigPatch,
    type Category,
    type KomgaLibrary,
    type PipelineConfig,
    type PipelineEvent,
    type PathValidationResult,
    type OptimizerValidationResult,
    type ValidatePathsResponse,
    type Tracker,
    type SyncRecord,
    type SyncSummary,
    type SyncScheduleConfig,
    type UnresolvedTitleDiagnostic,
    getNotificationConfig,
    saveNotificationConfig,
    sendTestNotification,
    getNotificationOutbox,
    type NtfyConfig,
    type OutboxEntry,
    getOpenTomeStatus,
    updateOpenTomeCatalogue,
    setOpenTomeConfig,
    type OpenTomeStatus,
    getAutoAcquireSettings,
    putAutoAcquireSettings,
    type AutoAcquireSettings,
    getMonitoringSettings,
    updateMonitoringSettings,
    type MonitoringSettings,
    type AutomationMode,
    getNotificationsHistory,
    resolveNotificationEvent,
    type NotificationEventItem,
    getLibraryIntegrity,
    type LibraryIntegrityReport,
    type IntegrityIssue,
    getStagingInventory,
    previewStagingCleanup,
    getStorageOptimizerStatus,
    getLibraryDuplicates,
    type StagingInventoryReport,
    type CleanupManifest,
    type StagingItem,
    type StorageOptimizerStatusDto,
    type LibraryDuplicateReportDto,
    runNamingAudit,
    getLatestNamingAudit,
    applyNamingCorrections,
    rollbackNamingBatch,
    getNamingBackups,
    type NamingAuditReport,
    type NamingAuditItem,
    type NamingCorrectionBackup,
    type NamingCorrectionResult,
    type NamingRollbackResult,
  } from '../lib/api';
  import { confirm } from '../lib/confirm.svelte';
  import { COVERS, display, MENUS, setDisplay, SIZES } from '../lib/display.svelte';
  import Icon, { type IconName } from '../lib/Icon.svelte';
  import Notice from '../lib/Notice.svelte';
  import { pref, setPref } from '../lib/prefs.svelte';
  import { KINDS, profile, setProfile, type Kind } from '../lib/reader';
  import ReaderPrefs from '../lib/ReaderPrefs.svelte';
  import { route } from '../lib/router.svelte';
  import { session, signOut } from '../lib/session.svelte';
  import { againTour } from '../lib/guide.svelte';
  import { toast } from '../lib/toast.svelte';
  import { ago } from '../lib/util';
  import { checkVersion, version as about } from '../lib/version.svelte';
  import { activeName, contrast, isHex, palette, PRESETS, saveTheme, sound, SPOTS, theme, type Theme } from '../lib/theme.svelte';

  // One group is on screen at a time. Which one is in the address: #/settings/<id>.
  const TABS: { id: string; label: string; icon: IconName; about: string }[] = $derived([
    { id: 'appearance', label: 'Appearance', icon: 'eye', about: 'Themes, colours and print effects.' },
    { id: 'reader', label: 'Reader', icon: 'book', about: 'How each kind of series is read.' },
    { id: 'library', label: 'Library', icon: 'bookmark', about: 'Categories, and the check for new chapters.' },
    { id: 'downloads', label: 'Downloads', icon: 'download', about: 'Chapters kept on this server.' },
    ...(session.admin ? [{ id: 'pipeline', label: 'Pipeline', icon: 'upload' as IconName, about: 'Komga publication, MangaBaka, and image optimization.' }] : []),
    ...(session.admin ? [{ id: 'notifications', label: 'Notifications', icon: 'bell' as IconName, about: 'ntfy notifications and delivery preferences.' }] : []),
    { id: 'sources', label: 'Sources', icon: 'puzzle', about: 'Where titles come from, and backups in and out.' },
    { id: 'trackers', label: 'Trackers', icon: 'link', about: 'Keep reading progress in step with a tracking site.' },
    { id: 'account', label: 'Account', icon: 'user', about: 'The password, signing out, and who else may use this Mango.' },
    { id: 'about', label: 'About', icon: 'info', about: 'The tour, this version, and newer ones.' },
  ]);

  // Deep-link route aliases for M12 Settings categories
  const ROUTE_ALIASES: Record<string, { tabId: string; pipelineSection?: string }> = {
    komga: { tabId: 'pipeline', pipelineSection: 'paths' },
    mangabaka: { tabId: 'pipeline', pipelineSection: 'metadata' },
    providers: { tabId: 'pipeline', pipelineSection: 'providers' },
    automation: { tabId: 'pipeline', pipelineSection: 'automation' },
    profiles: { tabId: 'pipeline', pipelineSection: 'automation' },
    opentome: { tabId: 'pipeline', pipelineSection: 'metadata' },
    maintenance: { tabId: 'pipeline', pipelineSection: 'maintenance' },
    storage: { tabId: 'pipeline', pipelineSection: 'diagnostics' },
    diagnostics: { tabId: 'pipeline', pipelineSection: 'diagnostics' },
  };

  let pipelineFilter = $state<string>('all');

  const rawSubroute = $derived(route.path.split('/')[2] || 'appearance');
  const resolvedTabId = $derived(ROUTE_ALIASES[rawSubroute]?.tabId ?? rawSubroute);
  const tab = $derived(TABS.find((entry) => entry.id === resolvedTabId) ?? TABS[0]);

  $effect(() => {
    const alias = ROUTE_ALIASES[rawSubroute];
    if (alias?.pipelineSection) {
      pipelineFilter = alias.pipelineSection;
    }
  });

  const currentCategory = $derived.by(() => {
    if (['appearance', 'reader', 'account', 'about'].includes(tab.id)) return 'general';
    if (tab.id === 'library') return 'library';
    if (tab.id === 'downloads') return 'acquisition';
    if (['trackers', 'notifications', 'sources'].includes(tab.id)) return 'tracking';
    if (tab.id === 'pipeline') {
      if (['providers', 'automation'].includes(pipelineFilter)) return 'acquisition';
      if (['maintenance', 'diagnostics'].includes(pipelineFilter)) return 'advanced';
      return 'library';
    }
    return 'general';
  });

  function selectCategory(catId: string) {
    if (catId === 'general') {
      location.hash = '#/settings/appearance';
    } else if (catId === 'library') {
      pipelineFilter = 'paths';
      location.hash = '#/settings/pipeline';
    } else if (catId === 'acquisition') {
      pipelineFilter = 'providers';
      location.hash = '#/settings/pipeline';
    } else if (catId === 'tracking') {
      location.hash = '#/settings/trackers';
    } else if (catId === 'advanced') {
      pipelineFilter = 'maintenance';
      location.hash = '#/settings/pipeline';
    }
  }

  // On a narrow screen the groups are a strip that scrolls sideways; keep the open one in view.
  let rail = $state<HTMLElement>();
  $effect(() => {
    void tab.id;
    const open = rail?.querySelector<HTMLElement>('[aria-current]');
    if (rail && open) rail.scrollTo({ left: open.offsetLeft - 24 });
  });

  // Body text needs this much contrast between ink and paper to stay readable.
  const ENOUGH = 7;
  // The reader's settings are held once for each kind of series; this is the one on show.
  let kind = $state<Kind>('manga');
  const FOLLOWS: [Theme['follow'], string][] = [
    ['system', 'System'],
    ['light', 'Day'],
    ['dark', 'Night'],
  ];

  let error = $state<string | null>(null);
  let version = $state('');

  const t = $derived(theme.value);
  const d = $derived(display.value);
  const names = [...Object.keys(PRESETS), 'Custom'];

  function change(patch: Partial<Theme>) {
    saveTheme({ ...t, ...patch }).catch((e) => (error = message(e)));
  }

  // ---------- custom colours ----------

  let paper = $state(theme.value.custom.paper);
  let ink = $state(theme.value.custom.ink);
  let agreed = $state(false);
  const ratio = $derived(isHex(paper) && isHex(ink) ? contrast(paper, ink) : 0);
  const weak = $derived(ratio < ENOUGH);

  const slotted = (slot: 'light' | 'dark', name: string): Partial<Theme> => (slot === 'dark' ? { dark: name } : { light: name });

  function useCustom() {
    // Low contrast is the owner's call to make, but never a silent one.
    if (weak && !agreed) {
      agreed = true;
      return;
    }
    agreed = false;
    // They go into whichever of the two themes is showing now.
    const night = t.follow === 'dark' || (t.follow === 'system' && activeName(t) === t.dark && t.dark !== t.light);
    change({ custom: { paper, ink }, ...slotted(night ? 'dark' : 'light', 'Custom') });
  }

  // ---------- import and export ----------

  let pasted = $state('');
  let copied = $state(false);
  const exported = $derived(JSON.stringify(t, null, 2));

  async function copy() {
    await navigator.clipboard.writeText(exported).catch(() => {});
    copied = true;
    setTimeout(() => (copied = false), 2000);
  }

  function importTheme() {
    error = null;
    let raw: unknown;
    try {
      raw = JSON.parse(pasted);
    } catch {
      error = 'That is not a theme: it does not read as JSON.';
      return;
    }
    const next = sound(raw);
    const worst = Math.min(...[next.light, next.dark].map((name) => contrast(palette(next, name).paper, palette(next, name).ink)));
    if (worst < ENOUGH && !agreed) {
      agreed = true;
      error = `This theme's ink on paper has a contrast of ${worst.toFixed(1)} to 1, under the ${ENOUGH} that reads well. Press Import again to use it anyway.`;
      return;
    }
    agreed = false;
    paper = next.custom.paper;
    ink = next.custom.ink;
    pasted = '';
    saveTheme(next).catch((e) => (error = message(e)));
  }

  // ---------- switches kept as settings ----------

  const flip = (key: string) => setPref(key, !pref(key, false)).catch((e) => (error = message(e)));

  function toggleIn(key: string, id: number) {
    const list = pref<number[]>(key, []);
    setPref(key, list.includes(id) ? list.filter((other) => other !== id) : [...list, id]).catch((e) => (error = message(e)));
  }

  // ---------- categories ----------

  let categories = $state<Category[]>([]);
  let fresh = $state('');
  let editing = $state<number | null>(null);
  let renamed = $state('');
  let categoryBusy = $state(false);

  async function loadCategories() {
    categories = (await get<{ categories: Category[] }>('/categories')).categories;
  }

  async function attempt(work: () => Promise<unknown>) {
    categoryBusy = true;
    error = null;
    try {
      await work();
      await loadCategories();
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      categoryBusy = false;
    }
  }

  function add(event: SubmitEvent) {
    event.preventDefault();
    attempt(async () => {
      await send('POST', '/categories', { name: fresh });
      toast.success(`Added category "${fresh}"`);
      fresh = '';
    });
  }

  function rename(event: SubmitEvent, id: number) {
    event.preventDefault();
    attempt(async () => {
      await send('PUT', `/categories/${id}`, { name: renamed });
      toast.success('Category renamed');
      editing = null;
    });
  }

  function move(index: number, by: number) {
    if (categoryBusy) return;
    const ids = categories.map((c) => c.id);
    const [id] = ids.splice(index, 1);
    ids.splice(index + by, 0, id);
    attempt(() => send('PUT', '/categories', { ids }));
  }

  async function remove(id: number) {
    const cat = categories.find((c) => c.id === id);
    const ok = await confirm({
      title: 'Delete Category?',
      message: `Delete category "${cat?.name ?? 'category'}"? Titles in this category will not be deleted.`,
      confirmText: 'Delete',
      danger: true,
    });
    if (!ok) return;
    attempt(async () => {
      await send('DELETE', `/categories/${id}`);
      toast.info('Category deleted');
    });
  }

  // Only the Library group shows categories, so they are read when it first opens.
  let asked = false;
  $effect(() => {
    if (tab.id !== 'library' || asked) return;
    asked = true;
    attempt(async () => {});
  });

  // ---------- trackers ----------

  const ANILIST_PIN = 'https://anilist.co/api/v2/oauth/pin';
  // Where a tracker that signs in through its own page sends the browser back to.
  const home = location.origin + location.pathname;

  let trackers = $state<Tracker[]>([]);
  let working = $state<number | null>(null);
  // What is typed into each tracker's sign-in form. Sent once, never kept.
  let typed = $state<Record<number, { name: string; secret: string }>>({});

  const clientOf = (t: Tracker) => pref(`trackers.client.${t.id}`, '');
  const setClient = (t: Tracker, value: string) => setPref(`trackers.client.${t.id}`, value.trim()).catch((e) => (error = message(e)));

  async function loadTrackers() {
    const answer = (await get<{ trackers: Tracker[] }>('/trackers')).trackers;
    for (const t of answer) typed[t.id] ??= { name: '', secret: '' };
    trackers = answer;
  }

  async function tracking(t: Tracker, work: () => Promise<unknown>) {
    working = t.id;
    error = null;
    try {
      await work();
      await loadTrackers();
    } catch (e) {
      error = message(e);
    }
    working = null;
  }

  function signIn(event: SubmitEvent, t: Tracker) {
    event.preventDefault();
    const form = typed[t.id];
    tracking(t, async () => {
      await send('POST', `/trackers/${t.id}/login`, t.login === 'token' ? { token: form.secret } : { username: form.name, password: form.secret });
      form.name = form.secret = '';
    });
  }

  // Leaves for the tracker's own page. It sends the browser back to `home` with its answer.
  function leaveFor(t: Tracker) {
    tracking(t, async () => {
      const { url } = await send<{ url: string }>('POST', `/trackers/${t.id}/authorize`, { clientId: clientOf(t), redirect: home });
      location.href = url;
    });
  }

  // For when the tracker's page did not lead back here: the address it led to, pasted in.
  function finish(event: SubmitEvent, t: Tracker) {
    event.preventDefault();
    const form = typed[t.id];
    tracking(t, async () => {
      const answer = new URLSearchParams(form.secret.includes('?') ? form.secret.slice(form.secret.indexOf('?') + 1) : form.secret);
      await send('POST', `/trackers/${t.id}/login`, { code: answer.get('code') ?? '', state: answer.get('state') ?? '' });
      form.secret = '';
    });
  }

  // Read when the group first opens. An answer a tracker's page sent back is handed over first.
  let askedTrackers = false;
  $effect(() => {
    if (tab.id !== 'trackers' || askedTrackers) return;
    askedTrackers = true;
    (async () => {
      const landed = sessionStorage.getItem('tracker.answer');
      sessionStorage.removeItem('tracker.answer');
      try {
        // Only MyAnimeList signs in this way.
        if (landed) await send('POST', '/trackers/1/login', JSON.parse(landed));
      } catch (e) {
        error = message(e);
      }
      await loadTrackers().catch((e) => (error = message(e)));
    })();
  });

  // ---------- account ----------

  let current = $state('');
  let renewed = $state('');
  let accountError = $state<string | null>(null);
  let changed = $state(false);

  async function changePassword(event: SubmitEvent) {
    event.preventDefault();
    accountError = null;
    changed = false;
    try {
      await send('PUT', '/auth/password', { current, password: renewed });
      current = renewed = '';
      changed = true;
    } catch (e) {
      accountError = message(e);
    }
  }

  // ---------- accounts, for an admin ----------

  type Account = { id: number; name: string; admin: boolean; createdAt: number; hasPassword: boolean };

  let users = $state<Account[]>([]);
  let usersError = $state<string | null>(null);
  let newcomer = $state({ name: '', password: '', admin: false });
  /** The account whose new password is being typed, and the one about to be removed. */
  let rekeyed = $state<number | null>(null);
  let rekey = $state('');
  let userBusy = $state(false);
  let inviteBusy = $state(false);

  async function withUsers(work: () => Promise<unknown>) {
    userBusy = true;
    usersError = null;
    try {
      await work();
      users = (await get<{ users: Account[] }>('/users')).users;
    } catch (e) {
      const msg = message(e);
      usersError = msg;
      toast.error(msg);
    } finally {
      userBusy = false;
    }
  }

  function addUser(event: SubmitEvent) {
    event.preventDefault();
    withUsers(async () => {
      await send('POST', '/users', newcomer);
      toast.success(`Created account "${newcomer.name}"`);
      newcomer = { name: '', password: '', admin: false };
    });
  }

  function setKey(event: SubmitEvent, id: number) {
    event.preventDefault();
    withUsers(async () => {
      await send('PUT', `/users/${id}`, { password: rekey });
      toast.success('Password changed');
      rekeyed = null;
      rekey = '';
    });
  }

  async function removeUser(id: number) {
    const user = users.find((u) => u.id === id);
    const ok = await confirm({
      title: 'Delete User Account?',
      message: `Delete account "${user?.name ?? 'user'}"? Their library and history will be permanently deleted.`,
      confirmText: 'Delete account',
      danger: true,
    });
    if (!ok) return;
    withUsers(async () => {
      await send('DELETE', `/users/${id}`);
      toast.info(`Deleted user account "${user?.name ?? ''}"`);
    });
  }

  async function toggleAdmin(user: Account) {
    const ok = await confirm({
      title: user.admin ? 'Revoke Admin Access?' : 'Grant Admin Access?',
      message: user.admin
        ? `Remove admin privileges from "${user.name}"?`
        : `Make "${user.name}" an administrator with full access to accounts and settings?`,
      confirmText: user.admin ? 'Revoke admin' : 'Make admin',
      danger: user.admin,
    });
    if (!ok) return;
    withUsers(async () => {
      await send('PUT', `/users/${user.id}`, { admin: !user.admin });
      toast.success(`Updated permissions for "${user.name}"`);
    });
  }

  let askedUsers = false;
  $effect(() => {
    if (tab.id !== 'account' || askedUsers || !session.login || !session.admin) return;
    askedUsers = true;
    withUsers(async () => {});
    if (session.signup) withInvites(async () => {});
  });

  // ---------- invitations, for an admin ----------

  type Invite = { id: number; note: string; createdAt: number; expiresAt: number; usedAt: number | null; usedBy: string | null; code?: string };

  let invites = $state<Invite[]>([]);
  let invitesError = $state<string | null>(null);
  let invitee = $state('');
  /** The invitation just made: the one moment its code can be shown. */
  let made = $state<Invite | null>(null);
  let linkCopied = $state(false);
  // The code stays after the #, which a browser sends to no server.
  const inviteLink = $derived(made?.code ? `${location.origin}${location.pathname}#/join/${made.code}` : '');

  async function withInvites(work: () => Promise<unknown>) {
    inviteBusy = true;
    invitesError = null;
    try {
      await work();
      invites = (await get<{ invites: Invite[] }>('/invites')).invites;
    } catch (e) {
      const msg = message(e);
      invitesError = msg;
      toast.error(msg);
    } finally {
      inviteBusy = false;
    }
  }

  function addInvite(event: SubmitEvent) {
    event.preventDefault();
    withInvites(async () => {
      made = await send<Invite>('POST', '/invites', { note: invitee });
      toast.success('Created invitation link');
      invitee = '';
      linkCopied = false;
    });
  }

  async function removeInvite(id: number) {
    const ok = await confirm({
      title: 'Revoke Invitation?',
      message: 'Revoke this invitation? It will no longer allow anyone to create an account.',
      confirmText: 'Revoke',
      danger: true,
    });
    if (!ok) return;
    if (made?.id === id) made = null;
    withInvites(async () => {
      await send('DELETE', `/invites/${id}`);
      toast.info('Invitation revoked');
    });
  }

  async function signOutTracker(t: Tracker) {
    const ok = await confirm({
      title: `Sign out of ${t.name}?`,
      message: `Sign out of ${t.name}? Reading progress will no longer synchronize with ${t.name}.`,
      confirmText: 'Sign out',
      danger: true,
    });
    if (!ok) return;
    tracking(t, async () => {
      await send('DELETE', `/trackers/${t.id}/login`);
      toast.info(`Signed out of ${t.name}`);
    });
  }

  async function copyInvite() {
    try {
      await navigator.clipboard.writeText(inviteLink);
      linkCopied = true;
    } catch {
      invitesError = 'The link could not be copied from here. Select it and copy it by hand.';
    }
  }

  function inviteState(invite: Invite): string {
    if (invite.usedAt) return `Used ${ago(invite.usedAt)}${invite.usedBy ? ` by ${invite.usedBy}` : ''}`;
    if (invite.expiresAt * 1000 < Date.now()) return 'Ran out unused';
    return `Open until ${new Date(invite.expiresAt * 1000).toLocaleDateString()}`;
  }

  // ---------- the solver ----------

  let solver = $state({ url: '', timeoutSecs: 60 });
  let solverError = $state<string | null>(null);
  let solverSaved = $state(false);
  let savingSolver = $state(false);

  async function saveSolver(event: SubmitEvent) {
    event.preventDefault();
    solverError = null;
    solverSaved = false;
    savingSolver = true;
    try {
      solver = await send('PUT', '/solver', solver);
      solverSaved = true;
      toast.success('Solver settings saved');
    } catch (e) {
      solverError = message(e);
      toast.error(solverError);
    } finally {
      savingSolver = false;
    }
  }

  let askedSolver = false;
  $effect(() => {
    if (tab.id !== 'sources' || askedSolver || !session.admin) return;
    askedSolver = true;
    get<typeof solver>('/solver')
      .then((kept) => (solver = kept))
      .catch((e) => (solverError = message(e)));
  });

  // ---------- acquisition pipeline ----------

  let pipeline = $state<PipelineConfig>({
    enabled: false,
    library_dir: null,
    chinese_library_dir: null,
    mangabaka_enabled: true,
    mangabaka_url: 'https://api.mangabaka.org/v1',
    has_mangabaka_token: false,
    komga_enabled: false,
    komga_url: 'http://127.0.0.1:25600',
    has_komga_api_key: false,
    komga_library_id: null,
    komga_chinese_library_id: null,
    komga_scan_enabled: true,
    optimize_enabled: false,
    optimizer_bin: null,
    target_height: 2400,
    image_quality: 90,
    min_savings_percent: 10,
  });
  let pipelineEvents = $state<PipelineEvent[]>([]);
  let pipelineLoading = $state(false);
  let pipelineSaving = $state(false);
  let validatingPaths = $state(false);
  let pipelineError = $state<string | null>(null);
  let mangabakaTesting = $state(false);
  let mangabakaTestResult = $state<{ ok: boolean; message: string } | null>(null);
  let komgaTesting = $state(false);
  let komgaTestResult = $state<{ ok: boolean; message: string } | null>(null);
  let komgaLibraries = $state<KomgaLibrary[]>([]);
  let discoveringKomgaLibraries = $state(false);

  let libraryDirInput = $state('');
  let chineseLibraryDirInput = $state('');
  let mangabakaUrlInput = $state('');
  let mangabakaTokenInput = $state('');
  let komgaUrlInput = $state('');
  let komgaApiKeyInput = $state('');
  let komgaLibraryIdInput = $state('');
  let komgaChineseLibraryIdInput = $state('');
  let komgaScanEnabledInput = $state(true);
  let optimizerBinInput = $state('');
  let targetHeightInput = $state(2400);
  let imageQualityInput = $state(90);
  let minSavingsPercentInput = $state(10);

  let libraryStatus = $state<PathValidationResult | null>(null);
  let chineseLibraryStatus = $state<PathValidationResult | null>(null);
  let optimizerStatus = $state<OptimizerValidationResult | null>(null);
  let bundledOptimizerPresent = $state(false);
  let resolvedOptimizerBin = $state<string | null>(null);

  let integrityReport = $state<LibraryIntegrityReport | null>(null);
  let integrityLoading = $state(false);
  let integritySeverityFilter = $state<string>('ALL');
  let integrityCategoryFilter = $state<string>('ALL');
  let integritySearchQuery = $state<string>('');

  async function runIntegrityAudit() {
    integrityLoading = true;
    try {
      integrityReport = await getLibraryIntegrity();
      if (integrityReport.totalIssues === 0) {
        toast.success('Library integrity audit complete: System healthy');
      } else {
        toast.info(`Library integrity audit: ${integrityReport.totalIssues} issues detected`);
      }
    } catch (e: any) {
      toast.error(message(e));
    } finally {
      integrityLoading = false;
    }
  }

  // ---------- Staging Lifecycle & Cleanup Preview (M7.1) ----------
  let stagingReport = $state<StagingInventoryReport | null>(null);
  let stagingLoading = $state<boolean>(false);
  let stagingManifest = $state<CleanupManifest | null>(null);
  let stagingManifestLoading = $state<boolean>(false);
  let stagingError = $state<string | null>(null);

  async function loadStagingInventory() {
    stagingLoading = true;
    stagingError = null;
    try {
      stagingReport = await getStagingInventory();
      toast.success(`Staging scanned: ${stagingReport.totalFiles} files (${sized(stagingReport.totalStagingBytes)})`);
    } catch (e: any) {
      stagingError = message(e);
      toast.error(stagingError);
    } finally {
      stagingLoading = false;
    }
  }

  async function loadCleanupPreview() {
    stagingManifestLoading = true;
    stagingError = null;
    try {
      stagingManifest = await previewStagingCleanup();
      toast.info(`Cleanup preview ready: ${stagingManifest.eligibleItems.length} eligible, ${stagingManifest.blockedItems.length} blocked`);
    } catch (e: any) {
      stagingError = message(e);
      toast.error(stagingError);
    } finally {
      stagingManifestLoading = false;
    }
  }

  // ---------- Optimizer Status & Library Duplicates (M8.6) ----------
  let storageOptimizerStatus = $state<StorageOptimizerStatusDto | null>(null);
  let optimizerStatusLoading = $state<boolean>(false);

  let duplicatesReport = $state<LibraryDuplicateReportDto | null>(null);
  let duplicatesLoading = $state<boolean>(false);
  let duplicatesError = $state<string | null>(null);

  async function loadOptimizerStatus() {
    optimizerStatusLoading = true;
    try {
      storageOptimizerStatus = await getStorageOptimizerStatus();
    } catch (e: any) {
      // Non-fatal probe
    } finally {
      optimizerStatusLoading = false;
    }
  }

  async function loadDuplicates() {
    duplicatesLoading = true;
    duplicatesError = null;
    try {
      duplicatesReport = await getLibraryDuplicates();
      toast.success(`Duplicate scan complete: ${duplicatesReport.totalDuplicateGroups} issue(s) detected`);
    } catch (e: any) {
      duplicatesError = message(e);
      toast.error(duplicatesError);
    } finally {
      duplicatesLoading = false;
    }
  }

  // ---------- acquisition providers (Nyaa / TorBox) ----------
  let acquisition = $state<AcquisitionConfig>({
    torboxEnabled: false,
    torboxUrl: 'https://api.torbox.app/v1/api',
    hasTorboxToken: false,
    nyaaEnabled: true,
    nyaaUrl: 'https://nyaa.si',
    nyaaCategory: '3_1',
  });
  let torboxEnabledInput = $state(false);
  let torboxUrlInput = $state('https://api.torbox.app/v1/api');
  let torboxTokenInput = $state('');
  let torboxTesting = $state(false);
  let torboxTestResult = $state<{ ok: boolean; message: string } | null>(null);

  let nyaaEnabledInput = $state(true);
  let nyaaUrlInput = $state('https://nyaa.si');
  let nyaaCategoryInput = $state('3_1');

  let madokamiEnabledInput = $state(true);
  let madokamiUrlInput = $state('https://manga.madokami.al');
  let madokamiUsernameInput = $state('');
  let madokamiPasswordInput = $state('');

  let operatingModeInput = $state<'simulation' | 'normal'>('simulation');
  let autoPublishManualInput = $state(false);

  async function toggleOperatingMode(target: 'simulation' | 'normal') {
    if (target === 'normal' && operatingModeInput !== 'normal') {
      const ok = await confirm({
        title: 'Switch to Normal Acquisition Mode?',
        message: 'In Normal Mode, explicit user-initiated downloads will initiate real network retrieval and TorBox/Madokami downloads. Automatic background acquisition remains governed by its separate kill switch.',
        confirmText: 'Enable Normal Mode',
        danger: true,
      });
      if (!ok) return;
    }
    operatingModeInput = target;
  }

  // Notifications tab state
  let ntfyConfig = $state<NtfyConfig | null>(null);
  let ntfyLoading = $state(false);
  let ntfySaving = $state(false);
  let ntfyTesting = $state(false);
  let ntfyTestResult = $state<{ success: boolean; message: string } | null>(null);
  let ntfyOutbox = $state<OutboxEntry[]>([]);
  let ntfyOutboxLoading = $state(false);
  let ntfyServerUrl = $state('https://ntfy.sh');
  let ntfyTopic = $state('');
  let ntfyToken = $state('');

  // Deduplicated Notification Events
  let notificationEvents = $state<NotificationEventItem[]>([]);
  let notificationEventsLoading = $state(false);
  let notificationEventsActiveOnly = $state(true);
  let resolvingEventId = $state<number | null>(null);

  let askedNotifications = false;
  $effect(() => {
    if (tab.id !== 'notifications' || askedNotifications || !session.admin) return;
    askedNotifications = true;
    loadNotifications();
  });

  async function loadNotifications() {
    ntfyLoading = true;
    try {
      ntfyConfig = await getNotificationConfig();
      ntfyServerUrl = ntfyConfig.serverUrl || 'https://ntfy.sh';
      ntfyTopic = ntfyConfig.topic || '';
      ntfyToken = ntfyConfig.hasToken ? '****' : '';
      loadOutbox();
      loadNotificationEvents();
    } catch (e: any) {
      toast.error(e?.message || 'Failed to load notification settings');
    } finally {
      ntfyLoading = false;
    }
  }

  async function loadNotificationEvents() {
    notificationEventsLoading = true;
    try {
      notificationEvents = await getNotificationsHistory(50, notificationEventsActiveOnly);
    } catch (e: any) {
      toast.error(message(e));
    } finally {
      notificationEventsLoading = false;
    }
  }

  async function handleResolveNotificationEvent(id: number) {
    resolvingEventId = id;
    try {
      await resolveNotificationEvent(id);
      toast.success(`Notification event #${id} resolved`);
      await loadNotificationEvents();
    } catch (e: any) {
      toast.error(message(e));
    } finally {
      resolvingEventId = null;
    }
  }

  async function loadOutbox() {
    ntfyOutboxLoading = true;
    try {
      ntfyOutbox = await getNotificationOutbox();
    } catch (e) {
      // ignore
    } finally {
      ntfyOutboxLoading = false;
    }
  }

  async function handleSaveNotifications() {
    if (!ntfyConfig) return;
    ntfySaving = true;
    try {
      const payload: Partial<NtfyConfig> = {
        enabled: ntfyConfig.enabled,
        serverUrl: ntfyServerUrl,
        topic: ntfyTopic,
        prefNewVolume: ntfyConfig.prefNewVolume,
        prefReleaseAvailable: ntfyConfig.prefReleaseAvailable,
        prefAcquisitionComplete: ntfyConfig.prefAcquisitionComplete,
        prefActionRequired: ntfyConfig.prefActionRequired,
      };
      if (ntfyToken !== '****') {
        payload.token = ntfyToken;
      }
      ntfyConfig = await saveNotificationConfig(payload);
      ntfyServerUrl = ntfyConfig.serverUrl;
      ntfyTopic = ntfyConfig.topic;
      ntfyToken = ntfyConfig.hasToken ? '****' : '';
      toast.success('Notification settings saved');
    } catch (e: any) {
      toast.error(e?.message || 'Failed to save notification settings');
    } finally {
      ntfySaving = false;
    }
  }

  async function handleTestNotification() {
    ntfyTesting = true;
    ntfyTestResult = null;
    try {
      const res = await sendTestNotification({
        serverUrl: ntfyServerUrl,
        topic: ntfyTopic,
        token: ntfyToken !== '****' ? ntfyToken : undefined,
      });
      if (res.success) {
        ntfyTestResult = { success: true, message: res.message || 'Test notification sent successfully!' };
        toast.success('Test notification sent successfully!');
      } else {
        ntfyTestResult = { success: false, message: res.error || 'Failed to send test notification' };
        toast.error(res.error || 'Failed to send test notification');
      }
    } catch (e: any) {
      const err = e?.message || 'Network error sending test notification';
      ntfyTestResult = { success: false, message: err };
      toast.error(err);
    } finally {
      ntfyTesting = false;
    }
  }

  let askedPipeline = false;
  $effect(() => {
    if (tab.id !== 'pipeline' || askedPipeline || !session.admin) return;
    askedPipeline = true;
    loadPipeline();
  });

  async function loadPipeline() {
    pipelineLoading = true;
    pipelineError = null;
    try {
      const [cfg, evts, acq] = await Promise.all([
        get<PipelineConfig>('/pipeline/config'),
        get<PipelineEvent[]>('/pipeline/events'),
        getAcquisitionSettings().catch(() => null),
      ]);
      pipeline = cfg;
      pipelineEvents = evts;
      libraryDirInput = cfg.library_dir ?? '';
      chineseLibraryDirInput = cfg.chinese_library_dir ?? '';
      mangabakaUrlInput = cfg.mangabaka_url ?? 'https://api.mangabaka.org/v1';
      mangabakaTokenInput = cfg.has_mangabaka_token ? '********' : '';
      komgaUrlInput = cfg.komga_url ?? 'http://127.0.0.1:25600';
      komgaApiKeyInput = cfg.has_komga_api_key ? '********' : '';
      komgaLibraryIdInput = cfg.komga_library_id ?? '';
      komgaChineseLibraryIdInput = cfg.komga_chinese_library_id ?? '';
      komgaScanEnabledInput = cfg.komga_scan_enabled ?? true;
      optimizerBinInput = cfg.optimizer_bin ?? '';
      targetHeightInput = cfg.target_height ?? 2400;
      imageQualityInput = cfg.image_quality ?? 90;
      minSavingsPercentInput = cfg.min_savings_percent ?? 10;
      libraryStatus = cfg.library_status ?? null;
      chineseLibraryStatus = cfg.chinese_library_status ?? null;
      optimizerStatus = cfg.optimizer_status ?? null;
      bundledOptimizerPresent = cfg.bundled_optimizer_present ?? false;
      resolvedOptimizerBin = cfg.resolved_optimizer_bin ?? null;

      if (acq) {
        acquisition = acq;
        torboxEnabledInput = acq.torboxEnabled ?? false;
        torboxUrlInput = acq.torboxUrl ?? 'https://api.torbox.app/v1/api';
        torboxTokenInput = acq.hasTorboxToken ? '********' : '';
        nyaaEnabledInput = acq.nyaaEnabled ?? true;
        nyaaUrlInput = acq.nyaaUrl ?? 'https://nyaa.si';
        nyaaCategoryInput = acq.nyaaCategory ?? '3_1';
        madokamiEnabledInput = acq.madokamiEnabled ?? true;
        madokamiUrlInput = acq.madokamiUrl ?? 'https://manga.madokami.al';
        madokamiUsernameInput = acq.hasMadokamiCredentials ? '********' : '';
        madokamiPasswordInput = acq.hasMadokamiCredentials ? '********' : '';
        operatingModeInput = (acq.operatingMode === 'normal' ? 'normal' : 'simulation');
        autoPublishManualInput = acq.autoPublishManualAcquisitions ?? false;
      }

      loadProfiles();
      loadSourceHealth();
      loadNormalizationHistory();
      loadOpenTome();
      loadAutoAcquireSettings();
      loadMonitoringSettings();
      runIntegrityAudit();
    } catch (e) {
      pipelineError = message(e);
    } finally {
      pipelineLoading = false;
    }
  }

  // ---------- M6.8: Guarded Auto-Acquisition ----------
  let autoAcquireSettings = $state<AutoAcquireSettings>({ enabled: false, dryRun: true });

  async function loadAutoAcquireSettings() {
    try {
      autoAcquireSettings = await getAutoAcquireSettings();
    } catch {
      autoAcquireSettings = { enabled: false, dryRun: true };
    }
  }

  async function toggleAutoAcquireGlobal() {
    const nextVal = !autoAcquireSettings.enabled;
    if (nextVal && !autoAcquireSettings.dryRun) {
      const ok = await confirm({
        title: 'Enable Live Auto-Acquisition?',
        message: 'Master switch will allow automatic acquisition execution for enabled titles. Dry-run is currently OFF. Ensure profiles and limits are properly configured.',
        confirmText: 'Enable Live Auto-Acquisition',
        danger: true,
      });
      if (!ok) return;
    }
    try {
      autoAcquireSettings = await putAutoAcquireSettings({ enabled: nextVal });
      toast.success(nextVal ? 'Global auto-acquisition enabled' : 'Global auto-acquisition disabled (Kill switch engaged)');
    } catch (e) {
      toast.error(`Failed to update auto-acquire settings: ${message(e)}`);
    }
  }

  async function toggleAutoAcquireDryRun() {
    const nextVal = !autoAcquireSettings.dryRun;
    if (!nextVal && autoAcquireSettings.enabled) {
      const ok = await confirm({
        title: 'Disable Simulation / Dry-Run Mode?',
        message: 'Disabling dry-run will allow eligible release targets on enabled titles to submit REAL acquisition jobs to TorBox.',
        confirmText: 'Disable Dry-Run',
        danger: true,
      });
      if (!ok) return;
    }
    try {
      autoAcquireSettings = await putAutoAcquireSettings({ dryRun: nextVal });
      toast.success(nextVal ? 'Dry-run simulation mode enabled' : 'Live acquisition mode enabled');
    } catch (e) {
      toast.error(`Failed to update dry-run mode: ${message(e)}`);
    }
  }

  // ---------- M10.1 / M11: Monitoring & Automation Settings ----------
  let monitoringSettings = $state<MonitoringSettings>({
    automationMode: 'monitor_only',
    intervalHours: 12,
    trackedStatuses: ['reading', 'planning'],
    lastCycleAt: null,
    autoAcquireEnabled: false,
    operatingMode: 'simulation',
    autoPublishManual: false,
  });
  let savingTrackedStatuses = $state(false);

  const monitoredSummaryText = $derived(
    !monitoringSettings.trackedStatuses || monitoringSettings.trackedStatuses.length === 0
      ? 'None selected'
      : monitoringSettings.trackedStatuses.map((s) => s.charAt(0).toUpperCase() + s.slice(1)).join(', ')
  );

  async function loadMonitoringSettings() {
    try {
      monitoringSettings = await getMonitoringSettings();
    } catch {
      // Keep defaults
    }
  }

  async function updateAutomationMode(mode: AutomationMode) {
    if (mode === 'automatic_acquisition') {
      const ok = await confirm({
        title: 'Enable Mode C — Automatic Acquisition?',
        message: 'Mode C will allow high-confidence, verified English releases for opted-in titles to be queued automatically. Global auto-acquire master switch must also be enabled.',
        confirmText: 'Enable Automatic Acquisition',
        danger: true,
      });
      if (!ok) return;
    }
    try {
      monitoringSettings = await updateMonitoringSettings({ automationMode: mode });
      toast.success(`Automation mode set to ${mode}`);
    } catch (e) {
      toast.error(`Failed to update automation mode: ${message(e)}`);
    }
  }

  async function toggleTrackedStatus(status: string) {
    if (savingTrackedStatuses) return;
    const previous = [...(monitoringSettings.trackedStatuses || [])];
    const current = new Set(monitoringSettings.trackedStatuses || []);
    if (current.has(status)) {
      current.delete(status);
    } else {
      current.add(status);
    }
    const updated = Array.from(current);
    // Optimistic update for immediate visual feedback
    monitoringSettings.trackedStatuses = updated;
    savingTrackedStatuses = true;
    try {
      const res = await updateMonitoringSettings({ trackedStatuses: updated });
      monitoringSettings = res;
      toast.success('Tracked list filters updated');
    } catch (e) {
      // Roll back to previously confirmed selection on error
      monitoringSettings.trackedStatuses = previous;
      toast.error(`Failed to update tracked list filters: ${message(e)}`);
    } finally {
      savingTrackedStatuses = false;
    }
  }

  async function updateMonitoringInterval(hours: number) {
    if (hours < 1 || hours > 168) return;
    try {
      monitoringSettings = await updateMonitoringSettings({ intervalHours: hours });
      toast.success(`Monitoring interval set to ${hours} hours`);
    } catch (e) {
      toast.error(`Failed to update interval: ${message(e)}`);
    }
  }

  // ---------- OpenTome Release Intelligence ----------
  let openTomeStatus = $state<OpenTomeStatus | null>(null);
  let openTomeUpdating = $state(false);

  async function loadOpenTome() {
    try {
      openTomeStatus = await getOpenTomeStatus();
    } catch {
      openTomeStatus = null;
    }
  }

  async function toggleOpenTome() {
    if (!openTomeStatus) return;
    try {
      await setOpenTomeConfig({ enabled: !openTomeStatus.enabled });
      await loadOpenTome();
      toast.success(openTomeStatus?.enabled ? 'OpenTome enabled' : 'OpenTome disabled');
    } catch {
      toast.error('Failed to change OpenTome setting');
    }
  }

  async function triggerOpenTomeUpdate(force = false) {
    if (openTomeUpdating) return;
    openTomeUpdating = true;
    try {
      const res = await updateOpenTomeCatalogue(force);
      await loadOpenTome();
      if (res.updated) {
        toast.success('OpenTome catalogue updated successfully');
      } else {
        toast.info('OpenTome catalogue is already up to date');
      }
    } catch (e: any) {
      toast.error(`OpenTome update failed: ${e?.message ?? e}`);
    } finally {
      openTomeUpdating = false;
    }
  }

  // ---------- Library Naming Standardization & Audit ----------
  let namingLibraryId = $state<string>('');
  let namingAuditing = $state<boolean>(false);
  let namingApplying = $state<boolean>(false);
  let namingRollingBack = $state<string | null>(null);
  let namingReport = $state<NamingAuditReport | null>(null);
  let selectedNamingBookIds = $state<Set<string>>(new Set());
  let namingBackups = $state<NamingCorrectionBackup[]>([]);
  let namingBackupsLoading = $state<boolean>(false);
  let namingError = $state<string | null>(null);
  let namingApplyDryRun = $state<boolean>(false);
  let namingUpdateCbz = $state<boolean>(false);

  async function loadNamingBackups() {
    namingBackupsLoading = true;
    try {
      namingBackups = await getNamingBackups();
    } catch (e) {
      console.warn('Failed to load naming backups:', e);
    } finally {
      namingBackupsLoading = false;
    }
  }

  async function handleRunNamingAudit() {
    namingAuditing = true;
    namingError = null;
    namingReport = null;
    selectedNamingBookIds = new Set();
    try {
      namingReport = await runNamingAudit(namingLibraryId || undefined);
      // Pre-select all safe items that are not already canonical
      const safeIds = new Set<string>();
      if (namingReport && namingReport.items) {
        for (const item of namingReport.items) {
          if (item.isSafe && !item.isAlreadyCanonical) {
            safeIds.add(item.bookId);
          }
        }
      }
      selectedNamingBookIds = safeIds;
      toast.success(`Naming audit complete: ${namingReport.totalBooks} books analyzed.`);
      await loadNamingBackups();
    } catch (e: any) {
      const msg = e?.message || 'Failed to execute naming audit.';
      namingError = msg;
      toast.error(msg);
    } finally {
      namingAuditing = false;
    }
  }

  async function handleApplyNamingCorrections() {
    if (!namingReport) return;
    if (selectedNamingBookIds.size === 0) {
      toast.error('No books selected for correction.');
      return;
    }
    namingApplying = true;
    namingError = null;
    try {
      const res = await applyNamingCorrections(
        namingReport.auditId,
        Array.from(selectedNamingBookIds),
        namingUpdateCbz,
        namingApplyDryRun,
      );
      if (res.isDryRun) {
        toast.info(`Dry run verified: ${res.totalRequested} books would be corrected.`);
      } else {
        toast.success(`Successfully applied and verified ${res.verifiedCount} book titles in Komga!`);
        await handleRunNamingAudit();
        await loadNamingBackups();
      }
    } catch (e: any) {
      const msg = e?.message || 'Failed to apply naming corrections.';
      namingError = msg;
      toast.error(msg);
    } finally {
      namingApplying = false;
    }
  }

  async function handleRollbackNaming(backupId: string) {
    namingRollingBack = backupId;
    try {
      const res = await rollbackNamingBatch(backupId);
      toast.success(`Rolled back ${res.rolledBackCount} book titles in Komga.`);
      await loadNamingBackups();
      await handleRunNamingAudit();
    } catch (e: any) {
      toast.error(e?.message || 'Failed to rollback naming batch.');
    } finally {
      namingRollingBack = null;
    }
  }

  // ---------- Library Normalization & Maintenance ----------
  let maintenanceLibraryId = $state<string>('');
  let maintenanceAnalyzing = $state<boolean>(false);
  let maintenanceNormalizing = $state<boolean>(false);
  let maintenanceRollingBack = $state<string | null>(null);
  let analysisSummary = $state<LibraryAnalysisSummary | null>(null);
  let selectedBookKeys = $state<Set<string>>(new Set());
  let normalizationHistory = $state<NormalizationHistoryRecord[]>([]);
  let historyLoading = $state<boolean>(false);
  let maintenanceError = $state<string | null>(null);

  async function loadNormalizationHistory() {
    historyLoading = true;
    try {
      normalizationHistory = await getNormalizationHistory(50);
    } catch (e) {
      console.warn('Failed to load normalization history:', e);
    } finally {
      historyLoading = false;
    }
  }

  async function handleRunDryRun() {
    if (!maintenanceLibraryId) {
      toast.error('Please select a library to analyze.');
      return;
    }
    maintenanceAnalyzing = true;
    maintenanceError = null;
    analysisSummary = null;
    selectedBookKeys = new Set();
    try {
      const summary = await runLibraryDryRun(maintenanceLibraryId);
      analysisSummary = summary;
      const newSelected = new Set<string>();
      for (const s of summary.series) {
        for (const b of s.books) {
          if (b.status === 'NEEDS_UPDATE') {
            newSelected.add(`${s.komgaSeriesId}:${b.komgaBookId}`);
          }
        }
      }
      selectedBookKeys = newSelected;
      toast.info(`Dry-run analysis complete: ${summary.needsUpdateCount} books need update, ${summary.alreadyNormalizedCount} already normalized, ${summary.skippedCount} skipped.`);
    } catch (e) {
      maintenanceError = message(e);
      toast.error(`Dry-run analysis failed: ${message(e)}`);
    } finally {
      maintenanceAnalyzing = false;
    }
  }

  async function handleNormalizeSelected() {
    if (!analysisSummary || selectedBookKeys.size === 0) return;
    const previewId = analysisSummary.previewId;
    maintenanceNormalizing = true;
    maintenanceError = null;

    const itemsToNormalize: NormalizeItemRequest[] = [];
    for (const s of analysisSummary.series) {
      for (const b of s.books) {
        const key = `${s.komgaSeriesId}:${b.komgaBookId}`;
        if (selectedBookKeys.has(key) && b.canonicalPath && b.mangabakaId) {
          itemsToNormalize.push({
            komgaBookId: b.komgaBookId,
          });
        }
      }
    }

    if (itemsToNormalize.length === 0 || itemsToNormalize.length > 25) {
      toast.error('Select between 1 and 25 eligible books per batch.');
      maintenanceNormalizing = false;
      return;
    }

    try {
      const approved = await confirm({ title: 'REAL MUTATION — normalize existing books?',
        message: `Replace metadata in ${itemsToNormalize.length} canonical CBZs using the reviewed dry run? Full original archives will be retained for rollback.`,
        confirmText: 'Normalize canonical files', danger: true });
      if (!approved) return;
      const res = await executeBatchNormalize(previewId, itemsToNormalize, false);
      toast.success(`Batch normalization finished: ${res.normalizedCount} normalized, ${res.alreadyNormalizedCount} already normalized, ${res.failedCount} failed.`);
      await loadNormalizationHistory();
      await handleRunDryRun();
    } catch (e) {
      maintenanceError = message(e);
      toast.error(`Normalization failed: ${message(e)}`);
    } finally {
      maintenanceNormalizing = false;
    }
  }

  async function handleNormalizeBatch25() {
    if (!analysisSummary) return;
    const previewId = analysisSummary.previewId;
    const candidates: NormalizeItemRequest[] = [];
    for (const s of analysisSummary.series) {
      for (const b of s.books) {
        if (b.status === 'NEEDS_UPDATE' && b.canonicalPath && b.mangabakaId) {
          candidates.push({
            komgaBookId: b.komgaBookId,
          });
          if (candidates.length >= 25) break;
        }
      }
      if (candidates.length >= 25) break;
    }

    if (candidates.length === 0) {
      toast.info('No eligible books needing update found.');
      return;
    }

    maintenanceNormalizing = true;
    try {
      const approved = await confirm({ title: 'REAL MUTATION — normalize batch?',
        message: `Replace metadata in ${candidates.length} canonical CBZs? This changes files; the dry run itself does not.`,
        confirmText: 'Normalize canonical files', danger: true });
      if (!approved) return;
      const res = await executeBatchNormalize(previewId, candidates, false);
      toast.success(`Normalized batch of ${res.normalizedCount} books (${res.failedCount} failed).`);
      await loadNormalizationHistory();
      await handleRunDryRun();
    } catch (e) {
      toast.error(`Batch normalize failed: ${message(e)}`);
    } finally {
      maintenanceNormalizing = false;
    }
  }

  async function handleRollbackRecord(recordId: string) {
    if (!await confirm({ title: 'REAL MUTATION — restore original archive?',
      message: 'Restore the full original CBZ from its verified backup? Books changed since normalization will be rejected.',
      confirmText: 'Restore original', danger: true })) return;
    maintenanceRollingBack = recordId;
    try {
      await rollbackNormalizationRecord(recordId);
      toast.success('Successfully reverted to original ComicInfo.xml!');
      await loadNormalizationHistory();
      if (analysisSummary) {
        await handleRunDryRun();
      }
    } catch (e) {
      toast.error(`Revert failed: ${message(e)}`);
    } finally {
      maintenanceRollingBack = null;
    }
  }

  // ---------- Acquisition Profiles & Sources ----------
  let profiles = $state<AcquisitionProfile[]>([]);
  let profilesLoading = $state(false);
  let sourceHealthReports = $state<SourceHealthReport[]>([]);
  let sourcesHealthLoading = $state(false);
  let editingProfile = $state<AcquisitionProfile | null>(null);
  let isNewProfile = $state(false);

  async function loadProfiles() {
    profilesLoading = true;
    try {
      profiles = await getAcquisitionProfiles();
    } catch (e) {
      // ignore
    } finally {
      profilesLoading = false;
    }
  }

  async function loadSourceHealth() {
    sourcesHealthLoading = true;
    try {
      sourceHealthReports = await getSourceHealthReports();
    } catch (e) {
      // ignore
    } finally {
      sourcesHealthLoading = false;
    }
  }

  function startNewProfile() {
    isNewProfile = true;
    editingProfile = {
      id: '',
      name: '',
      isDefault: false,
      language: 'en',
      preferredReleaseType: 'volume',
      sourceSelection: 'smart',
      preferredSources: ['nyaa', 'mangadot'],
      preferredUploaders: [],
      minSeeders: 1,
      preferDigital: true,
      trustedOnly: false,
      collectionPolicy: 'smart',
      maxUnnecessaryDuplicateCoverage: 3,
      maxAcquisitionSizeBytes: null,
      maxPlanSizeBytes: null,
      qualityPreference: 'best',
      allowMixedCollections: false,
      allowZeroSeeders: 'no',
      allowUnsupportedEditions: false,
      autoAcquire: false,
      createdAt: 0,
      updatedAt: 0,
    };
  }

  function startEditProfile(prof: AcquisitionProfile) {
    isNewProfile = false;
    editingProfile = { ...prof };
  }

  async function saveEditingProfile() {
    if (!editingProfile || !editingProfile.name.trim()) {
      toast.error('Profile name is required');
      return;
    }
    try {
      if (isNewProfile) {
        await createAcquisitionProfile(editingProfile);
        toast.success(`Created profile "${editingProfile.name}"`);
      } else {
        await updateAcquisitionProfile(editingProfile.id, editingProfile);
        toast.success(`Updated profile "${editingProfile.name}"`);
      }
      editingProfile = null;
      await loadProfiles();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function handleDuplicateProfile(id: string) {
    try {
      const dup = await duplicateAcquisitionProfile(id);
      toast.success(`Duplicated profile as "${dup.name}"`);
      await loadProfiles();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function handleDeleteProfile(prof: AcquisitionProfile) {
    if (prof.isDefault) {
      toast.error('Cannot delete the default profile');
      return;
    }
    const ok = await confirm({
      title: `Delete profile "${prof.name}"?`,
      message: 'Any series assigned to this profile will revert to the default profile.',
      confirm: 'Delete',
      danger: true,
    });
    if (!ok) return;
    try {
      await deleteAcquisitionProfile(prof.id);
      toast.success(`Deleted profile "${prof.name}"`);
      await loadProfiles();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function handleSetDefaultProfile(prof: AcquisitionProfile) {
    try {
      await updateAcquisitionProfile(prof.id, { isDefault: true });
      toast.success(`Set "${prof.name}" as default profile`);
      await loadProfiles();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function validatePaths() {
    validatingPaths = true;
    try {
      const res = await send<ValidatePathsResponse>('POST', '/pipeline/validate-paths', {
        library_dir: libraryDirInput.trim() || null,
        chinese_library_dir: chineseLibraryDirInput.trim() || null,
        optimizer_bin: optimizerBinInput.trim() || null,
      });
      libraryStatus = res.library;
      chineseLibraryStatus = res.chinese_library;
      optimizerStatus = res.optimizer;
      bundledOptimizerPresent = res.bundled_optimizer_present;
      resolvedOptimizerBin = res.resolved_optimizer_bin;
      if (res.library.writable) {
        toast.success('Library destination is valid and writable inside container');
      } else {
        toast.error(`Library path issue: ${res.library.message}`);
      }
    } catch (e) {
      toast.error(message(e));
    } finally {
      validatingPaths = false;
    }
  }

  async function savePipeline() {
    if (!pipeline) return;
    pipelineSaving = true;
    pipelineError = null;
    try {
      const patch: PipelineConfig = {
        ...pipeline,
        library_dir: libraryDirInput.trim() || null,
        chinese_library_dir: chineseLibraryDirInput.trim() || null,
        mangabaka_url: mangabakaUrlInput.trim() || null,
        mangabaka_token: mangabakaTokenInput.trim() || null,
        komga_url: komgaUrlInput.trim() || null,
        komga_api_key: komgaApiKeyInput.trim() || null,
        komga_library_id: komgaLibraryIdInput.trim() || null,
        komga_chinese_library_id: komgaChineseLibraryIdInput.trim() || null,
        komga_scan_enabled: komgaScanEnabledInput,
        optimizer_bin: optimizerBinInput.trim() || null,
        target_height: Number(targetHeightInput) || 2400,
        image_quality: Number(imageQualityInput) || 90,
        min_savings_percent: Number(minSavingsPercentInput) >= 0 ? Number(minSavingsPercentInput) : 10,
      };
      const acqPatch: AcquisitionConfigPatch = {
        torboxEnabled: torboxEnabledInput,
        torboxUrl: torboxUrlInput.trim() || undefined,
        torboxToken: torboxTokenInput === 'CLEAR' ? '' : (torboxTokenInput && torboxTokenInput !== '********' ? torboxTokenInput.trim() : undefined),
        nyaaEnabled: nyaaEnabledInput,
        nyaaUrl: nyaaUrlInput.trim() || undefined,
        nyaaCategory: nyaaCategoryInput.trim() || undefined,
        madokamiEnabled: madokamiEnabledInput,
        madokamiUrl: madokamiUrlInput.trim() || undefined,
        madokamiUsername: madokamiUsernameInput === 'CLEAR' ? '' : (madokamiUsernameInput && madokamiUsernameInput !== '********' ? madokamiUsernameInput.trim() : undefined),
        madokamiPassword: madokamiPasswordInput === 'CLEAR' ? '' : (madokamiPasswordInput && madokamiPasswordInput !== '********' ? madokamiPasswordInput.trim() : undefined),
        operatingMode: operatingModeInput,
        autoPublishManualAcquisitions: autoPublishManualInput,
      };
      await Promise.all([
        send('PUT', '/pipeline/config', patch),
        putAcquisitionSettings(acqPatch),
      ]);
      toast.success('Acquisition pipeline settings saved');
      await loadPipeline();
    } catch (e) {
      pipelineError = message(e);
      toast.error(message(e));
    } finally {
      pipelineSaving = false;
    }
  }

  async function testTorBox() {
    torboxTesting = true;
    torboxTestResult = null;
    try {
      const token = torboxTokenInput && torboxTokenInput !== '********' ? torboxTokenInput.trim() : undefined;
      const res = await testTorBoxConnection(torboxUrlInput.trim() || undefined, token);
      torboxTestResult = { ok: res.success, message: res.message };
      if (res.success) {
        acquisition.torboxValidated = true;
        acquisition.torboxStatusMessage = res.message;
        toast.success(res.message);
      } else {
        acquisition.torboxValidated = false;
        acquisition.torboxStatusMessage = res.message;
        toast.error(res.message);
      }
    } catch (e) {
      const err = message(e);
      torboxTestResult = { ok: false, message: err };
      acquisition.torboxValidated = false;
      acquisition.torboxStatusMessage = err;
      toast.error(err);
    } finally {
      torboxTesting = false;
    }
  }

  async function testMangaBaka() {
    mangabakaTesting = true;
    mangabakaTestResult = null;
    try {
      const res = await send<{ message: string; validated?: boolean }>('POST', '/pipeline/test-mangabaka', {
        url: mangabakaUrlInput.trim() || null,
        token: mangabakaTokenInput.trim() || null,
      });
      mangabakaTestResult = { ok: true, message: res.message };
      if (pipeline) pipeline.mangabaka_validated = true;
    } catch (e) {
      mangabakaTestResult = { ok: false, message: message(e) };
      if (pipeline) pipeline.mangabaka_validated = false;
    } finally {
      mangabakaTesting = false;
    }
  }

  async function testKomga() {
    komgaTesting = true;
    komgaTestResult = null;
    try {
      const res = await send<{ message: string }>('POST', '/pipeline/test-komga', {
        url: komgaUrlInput.trim() || null,
        token: komgaApiKeyInput.trim() || null,
      });
      komgaTestResult = { ok: true, message: res.message };
      discoverKomgaLibraries().catch(() => {});
    } catch (e) {
      komgaTestResult = { ok: false, message: message(e) };
    } finally {
      komgaTesting = false;
    }
  }

  async function discoverKomgaLibraries() {
    discoveringKomgaLibraries = true;
    try {
      const libs = await send<KomgaLibrary[]>('POST', '/pipeline/discover-komga-libraries', {
        url: komgaUrlInput.trim() || null,
        token: komgaApiKeyInput.trim() || null,
      });
      komgaLibraries = libs;
      if (libs.length > 0) {
        toast.success(`Discovered ${libs.length} Komga ${libs.length === 1 ? 'library' : 'libraries'}`);
      } else {
        toast.info('No libraries returned by Komga');
      }
    } catch (e) {
      toast.error(`Library discovery failed: ${message(e)}`);
    } finally {
      discoveringKomgaLibraries = false;
    }
  }

  // ---------- reading progress sync ----------
  let syncRunning = $state(false);
  let syncLoading = $state(false);
  let syncSummary = $state<SyncSummary | null>(null);
  let syncHistory = $state<SyncRecord[]>([]);
  let syncError = $state<string | null>(null);
  let syncSchedule = $state<SyncScheduleConfig>({
    enabled: false,
    intervalHours: 24,
    lastRun: null,
  });
  let askedSyncHistory = false;

  async function loadSyncSchedule() {
    try {
      syncSchedule = await getSyncSchedule();
    } catch {}
  }

  async function toggleSyncSchedule() {
    syncSchedule.enabled = !syncSchedule.enabled;
    await saveSchedule();
  }

  async function saveSchedule() {
    try {
      await saveSyncSchedule(syncSchedule);
      toast.success('Sync schedule updated');
    } catch (e) {
      toast.error(`Failed to update schedule: ${message(e)}`);
    }
  }

  let syncDiagnostics = $state<UnresolvedTitleDiagnostic[]>([]);
  let manualMappingModal = $state<{
    open: boolean;
    komgaSeriesId: string;
    komgaTitle: string;
    tracker: string;
    trackerId: string;
    editionId: string;
  }>({
    open: false,
    komgaSeriesId: '',
    komgaTitle: '',
    tracker: 'anilist',
    trackerId: '',
    editionId: '',
  });

  async function loadSyncDiagnostics() {
    try {
      syncDiagnostics = await getUnresolvedDiagnostics();
    } catch {}
  }

  async function handleSetManualMapping() {
    if (!manualMappingModal.trackerId.trim()) return;
    try {
      await setManualSyncMapping({
        komgaSeriesId: manualMappingModal.komgaSeriesId,
        tracker: manualMappingModal.tracker,
        trackerId: manualMappingModal.trackerId.trim(),
        editionId: manualMappingModal.editionId.trim() || null,
        trackerTitle: manualMappingModal.komgaTitle,
      });
      toast.success('Manual mapping saved');
      manualMappingModal.open = false;
      await loadSyncDiagnostics();
      await loadSyncHistory();
    } catch (e) {
      toast.error(`Failed to save manual mapping: ${message(e)}`);
    }
  }

  async function handleResetMapping(komgaSeriesId: string, tracker: string) {
    try {
      await resetSyncMapping(komgaSeriesId, tracker);
      toast.success('Mapping reset to automatic resolution');
      await loadSyncDiagnostics();
      await loadSyncHistory();
    } catch (e) {
      toast.error(`Failed to reset mapping: ${message(e)}`);
    }
  }

  async function loadSyncHistory() {
    try {
      syncHistory = await getSyncHistory(50);
      const status = await getSyncStatus();
      syncRunning = status.running;
      pipeline.komga_enabled = status.komga_enabled;
      pipeline.has_komga_api_key = status.has_komga_api_key;
      pipeline.mangabaka_enabled = status.mangabaka_enabled;
      pipeline.has_mangabaka_token = status.has_mangabaka_token;
      pipeline.mangabaka_validated = status.mangabaka_validated;
      void loadSyncSchedule();
      void loadSyncDiagnostics();
    } catch (e) {
      syncError = message(e);
    }
  }

  async function triggerDryRun() {
    syncLoading = true;
    syncError = null;
    try {
      syncSummary = await runSyncDryRun();
      await loadSyncHistory();
      toast.success('Dry run evaluation completed');
    } catch (e) {
      syncError = message(e);
      toast.error(syncError);
    } finally {
      syncLoading = false;
    }
  }

  async function triggerLiveSync() {
    const ok = await confirm({
      title: 'Synchronize Reading Progress?',
      message: 'This will update your reading progress on external trackers (AniList, MangaBaka) to match your completed books in Komga. Progress only moves forward and never decreases.',
      confirmText: 'Sync Now',
    });
    if (!ok) return;

    syncLoading = true;
    syncError = null;
    try {
      syncSummary = await runSync();
      await loadSyncHistory();
      toast.success('Reading progress synchronization completed');
    } catch (e) {
      syncError = message(e);
      toast.error(syncError);
    } finally {
      syncLoading = false;
    }
  }

  let auditFilter = $state<'all' | 'latest'>('all');

  const latestRunTimestamp = $derived(syncHistory[0]?.timestamp);

  function isLatestRun(rec: SyncRecord): boolean {
    if (syncSummary && syncSummary.records.some((r) => r.id === rec.id)) return true;
    if (!latestRunTimestamp) return false;
    return Math.abs(rec.timestamp - latestRunTimestamp) <= 30;
  }

  const latestRunRecordsCount = $derived(
    syncHistory.filter((r) => isLatestRun(r)).length
  );

  const filteredSyncHistory = $derived(
    auditFilter === 'latest'
      ? syncHistory.filter((r) => isLatestRun(r))
      : syncHistory
  );

  $effect(() => {
    if (tab.id !== 'trackers' || askedSyncHistory) return;
    askedSyncHistory = true;
    void loadSyncHistory();
    if (session.admin && !askedPipeline) {
      askedPipeline = true;
      void loadPipeline();
    }
  });

  // ---------- version ----------

  const v = $derived(about.info);
  let checking = $state(false);
  let versionError = $state<string | null>(null);

  async function check() {
    checking = true;
    versionError = null;
    try {
      await checkVersion();
    } catch (e) {
      versionError = message(e);
    }
    checking = false;
  }

  const day = (iso: string | null) => (iso ? new Date(iso).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' }) : '');

  let localDir = $state('');
  get<{ version: string; localDir: string }>('/health')
    .then((health) => ((version = health.version), (localDir = health.localDir)))
    .catch(() => {});

  // What "Export my files" would hold, asked for when the Downloads group opens.
  type FilesSize = { titles: number; chapters: number; bytes: number };
  let files = $state<FilesSize | null>(null);
  $effect(() => {
    if (tab.id !== 'downloads') return;
    get<FilesSize>('/export/files/size')
      .then((size) => (files = size))
      .catch(() => (files = null));
  });
  const counted = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;
  const sized = (bytes: number) => (bytes >= 1 << 30 ? `${(bytes / (1 << 30)).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / (1 << 20)))} MB`);
</script>

{#snippet toggle(on: boolean, label: string, press: () => void)}
  <button class="switch" role="switch" aria-checked={on} aria-label={label} onclick={press}>
    <span class="track"><span class="knob"></span></span>
    <span class="state">{on ? 'On' : 'Off'}</span>
  </button>
{/snippet}

{#snippet swatches(slot: 'light' | 'dark', label: string, hint: string)}
  <div class="set stack" role="group" aria-label={label}>
    <div class="what"><strong>{label}</strong><span>{hint}</span></div>
    <div class="how">
      {#each names as name (name)}
        {@const colours = palette(t, name)}
        <button
          class="btn swatch"
          aria-pressed={t[slot] === name}
          style:background={colours.paper}
          style:color={colours.ink}
          style:border-color={colours.ink}
          onclick={() => change(slotted(slot, name))}
        >
          {#if t[slot] === name}<Icon name="check" size={16} stroke={4} />{/if}{name}
        </button>
      {/each}
    </div>
  </div>
{/snippet}

<div class="page">
  <div class="page-head">
    <div class="head-left">
      <h1 class="display">SETTINGS</h1>
      <div class="pill-group category-switcher" role="tablist" aria-label="Settings categories">
        <button
          class="pill"
          class:active={currentCategory === 'general'}
          onclick={() => selectCategory('general')}
          role="tab"
          aria-selected={currentCategory === 'general'}
        >
          <Icon name="sliders" size={14} /> General
        </button>
        <button
          class="pill"
          class:active={currentCategory === 'library'}
          onclick={() => selectCategory('library')}
          role="tab"
          aria-selected={currentCategory === 'library'}
        >
          <Icon name="book" size={14} /> Library & Metadata
        </button>
        <button
          class="pill"
          class:active={currentCategory === 'acquisition'}
          onclick={() => selectCategory('acquisition')}
          role="tab"
          aria-selected={currentCategory === 'acquisition'}
        >
          <Icon name="download" size={14} /> Acquisition
        </button>
        <button
          class="pill"
          class:active={currentCategory === 'tracking'}
          onclick={() => selectCategory('tracking')}
          role="tab"
          aria-selected={currentCategory === 'tracking'}
        >
          <Icon name="link" size={14} /> Tracking & Updates
        </button>
        <button
          class="pill"
          class:active={currentCategory === 'advanced'}
          onclick={() => selectCategory('advanced')}
          role="tab"
          aria-selected={currentCategory === 'advanced'}
        >
          <Icon name="puzzle" size={14} /> Advanced
        </button>
      </div>
    </div>
  </div>

  <div class="layout">
    <nav class="rail" aria-label="Settings groups" bind:this={rail}>
      {#each TABS as entry (entry.id)}
        <a href="#/settings/{entry.id}" aria-current={entry.id === tab.id ? 'page' : undefined}>
          <Icon name={entry.icon} size={18} />
          <span>{entry.label}</span>
          {#if entry.id === 'about' && v?.updateAvailable}<span class="tag spot">New</span>{/if}
        </a>
      {/each}
    </nav>

    <div class="pane">
      <div class="section-head">
        <h2 class="display">{tab.label.toUpperCase()}</h2>
        <span class="muted">{tab.about}</span>
      </div>

      <Notice {error} />

      {#if tab.id === 'appearance'}
        <section class="card">
          <h3>Theme</h3>
          <div class="set">
            <div class="what"><strong>Show</strong><span>System follows the device between the day and the night theme.</span></div>
            <div class="how">
              <div class="seg" role="group" aria-label="Which theme shows">
                {#each FOLLOWS as [value, label] (value)}
                  <button aria-pressed={t.follow === value} onclick={() => change({ follow: value })}>{label}</button>
                {/each}
              </div>
            </div>
          </div>
          {@render swatches('light', 'Day theme', 'Used when the device is in light mode.')}
          {@render swatches('dark', 'Night theme', 'Used when the device is in dark mode.')}
          <div class="set stack" role="group" aria-label="Spot colour">
            <div class="what"><strong>Spot colour</strong><span>The one accent: main buttons, badges, highlights.</span></div>
            <div class="how">
              {#each Object.entries(SPOTS) as [name, colour] (name)}
                <button class="btn swatch" aria-pressed={t.spot.toUpperCase() === colour} style:background={colour} style:color="#111111" style:border-color="var(--ink)" onclick={() => change({ spot: colour })}>
                  {#if t.spot.toUpperCase() === colour}<Icon name="check" size={16} stroke={4} />{/if}{name}
                </button>
              {/each}
              <label class="btn picker">
                Your own
                <input type="color" value={t.spot} onchange={(e) => change({ spot: e.currentTarget.value })} />
              </label>
            </div>
          </div>
        </section>

        <section class="card">
          <h3>Custom colours</h3>
          <div class="set stack" role="group" aria-label="Custom colours">
            <div class="what"><strong>Paper and ink</strong><span>They go into whichever of the two themes is showing now.</span></div>
            <div class="how">
              <label class="btn picker">Paper <input type="color" bind:value={paper} oninput={() => (agreed = false)} /></label>
              <label class="btn picker">Ink <input type="color" bind:value={ink} oninput={() => (agreed = false)} /></label>
              <span class="sample" style:background={paper} style:color={ink} style:border-color={ink}>Ink on paper, {ratio.toFixed(1)} to 1</span>
              <button class="btn ink" onclick={useCustom}>
                {weak && agreed ? 'Use them anyway' : 'Use these colours'}
              </button>
            </div>
            {#if weak}
              <p class="warn" role="status">
                <strong>Hard to read.</strong> Text wants a contrast of at least {ENOUGH} to 1 between ink and paper; these give {ratio.toFixed(1)}.
                {agreed ? 'Press again to use them anyway.' : ''}
              </p>
            {/if}
          </div>
        </section>

        <section class="card">
          <h3>Print effects</h3>
          <div class="set">
            <div class="what"><strong>Hard shadows</strong><span>The offset ink shadow under covers, panels and buttons.</span></div>
            <div class="how">{@render toggle(t.shadows, 'Hard shadows', () => change({ shadows: !t.shadows }))}</div>
          </div>
          <div class="set">
            <div class="what"><strong>Halftone dots</strong><span>The dotted screen behind covers and empty spaces.</span></div>
            <div class="how">{@render toggle(t.dots, 'Halftone dots', () => change({ dots: !t.dots }))}</div>
          </div>
        </section>

        <section class="card">
          <h3>This device</h3>
          <div class="set">
            <div class="what"><strong>Size</strong><span>Text and controls together. Like the two below, kept on this device alone: a phone and a desk want different answers.</span></div>
            <div class="how">
              <div class="seg" role="group" aria-label="Size of text and controls">
                {#each SIZES as [value, label] (value)}
                  <button aria-pressed={d.size === value} onclick={() => setDisplay({ size: value })}>{label}</button>
                {/each}
              </div>
            </div>
          </div>
          <div class="set">
            <div class="what"><strong>Covers</strong><span>Small puts three across a phone's screen, the others two.</span></div>
            <div class="how">
              <div class="seg" role="group" aria-label="Size of covers">
                {#each COVERS as [value, label] (value)}
                  <button aria-pressed={d.covers === value} onclick={() => setDisplay({ covers: value })}>{label}</button>
                {/each}
              </div>
            </div>
          </div>
          <div class="set">
            <div class="what"><strong>Menu</strong><span>Along the bottom it is under the thumb. Automatic puts it there on a narrow screen.</span></div>
            <div class="how">
              <div class="seg" role="group" aria-label="Where the menu sits">
                {#each MENUS as [value, label] (value)}
                  <button aria-pressed={d.tabs === value} onclick={() => setDisplay({ tabs: value })}>{label}</button>
                {/each}
              </div>
            </div>
          </div>
          <div class="set">
            <div class="what">
              <strong>On a phone's home screen</strong>
              <span>Add Mango to it from the browser's share menu, and it opens as an app of its own, without the address bar.</span>
            </div>
          </div>
        </section>

        <section class="card">
          <h3>Move a theme between devices</h3>
          <div class="set stack">
            <label class="swap">
              <span>This theme</span>
              <textarea readonly rows="6" value={exported}></textarea>
            </label>
            <button class="btn" onclick={copy}>{copied ? 'Copied' : 'Copy'}</button>
          </div>
          <div class="set stack">
            <label class="swap">
              <span>A theme to bring in</span>
              <textarea rows="6" bind:value={pasted} oninput={() => (agreed = false)} placeholder="Paste a theme here"></textarea>
            </label>
            <button class="btn ink" disabled={!pasted.trim()} onclick={importTheme}>Import</button>
          </div>
        </section>
      {:else if tab.id === 'reader'}
        {@const name = KINDS.find((entry) => entry.id === kind)?.name.toLowerCase()}
        <section class="card">
          <h3>Kind of series</h3>
          <div class="set">
            <div class="what">
              <strong>Settings for</strong>
              <span>Mango tells the kinds apart by a title's tags and its source. From inside the reader a title can be told what it is, and keep settings of its own.</span>
            </div>
            <div class="how">
              <div class="seg" role="group" aria-label="Kind of series">
                {#each KINDS as entry (entry.id)}
                  <button aria-pressed={kind === entry.id} onclick={() => (kind = entry.id)}>{entry.name}</button>
                {/each}
              </div>
            </div>
          </div>
          <div class="set">
            <div class="what"><strong>Start over</strong><span>Puts every {name} setting below back to how Mango ships it.</span></div>
            <div class="how"><button class="btn" onclick={() => setProfile(kind, null)}>Reset {name}</button></div>
          </div>
        </section>
        <ReaderPrefs value={profile(kind)} onchange={(patch) => setProfile(kind, patch)} />
      {:else if tab.id === 'library'}
        <section class="card">
          <h3>Categories</h3>
          <ul class="rows">
            {#each categories as c, index (c.id)}
              <li>
                {#if editing === c.id}
                  <form class="main rename" onsubmit={(e) => rename(e, c.id)}>
                    <label class="field"><span class="sr">New name for {c.name}</span><input bind:value={renamed} required maxlength="60" /></label>
                    <button class="btn ink" disabled={savingSolver}>
                {#if savingSolver}
                  <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                {/if}
                Save
              </button>
                    <button class="btn" type="button" onclick={() => (editing = null)}>Cancel</button>
                  </form>
                {:else}
                  <span class="main"><strong>{c.name}</strong></span>
                  <span class="tools">
                    <button class="btn icon" aria-label="Move {c.name} up" disabled={index === 0 || categoryBusy} onclick={() => move(index, -1)}><Icon name="up" /></button>
                    <button class="btn icon" aria-label="Move {c.name} down" disabled={index === categories.length - 1 || categoryBusy} onclick={() => move(index, 1)}><Icon name="down" /></button>
                    <button class="btn" disabled={categoryBusy} onclick={() => ((editing = c.id), (renamed = c.name))}>Rename</button>
                    <button class="btn" disabled={categoryBusy} onclick={() => remove(c.id)}><Icon name="trash" size={18} />Remove</button>
                  </span>
                {/if}
              </li>
            {/each}
          </ul>
          <form class="set" onsubmit={add}>
            <label class="field grow"><span class="sr">Name of a new category</span><Icon name="plus" size={18} stroke={3} /><input bind:value={fresh} placeholder="New category" required maxlength="60" /></label>
            <button class="btn ink" disabled={categoryBusy}>
              {#if categoryBusy}
                <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
              {/if}
              Add category
            </button>
          </form>
        </section>

        <section class="card">
          <h3>New chapters</h3>
          <div class="set">
            <div class="what"><strong>Check the library</strong><span>How often Mango looks for new chapters by itself.</span></div>
            <div class="how">
              <select class="btn" aria-label="Check the library for new chapters" value={String(pref('updates.hours', 12))} onchange={(e) => setPref('updates.hours', Number(e.currentTarget.value))}>
                <option value="0">Only when asked</option>
                <option value="6">Every 6 hours</option>
                <option value="12">Every 12 hours</option>
                <option value="24">Once a day</option>
                <option value="48">Every 2 days</option>
              </select>
            </div>
          </div>
          <div class="set">
            <div class="what"><strong>Skip completed titles</strong><span>A finished series gets no more chapters.</span></div>
            <div class="how">{@render toggle(pref('updates.skipCompleted', false), 'Skip completed titles', () => flip('updates.skipCompleted'))}</div>
          </div>
          <div class="set">
            <div class="what"><strong>Skip titles with unread chapters</strong><span>Catch up first, then look for more.</span></div>
            <div class="how">{@render toggle(pref('updates.skipUnread', false), 'Skip titles with unread chapters', () => flip('updates.skipUnread'))}</div>
          </div>
          <div class="set">
            <div class="what">
              <strong>Wait for the day a chapter is expected</strong>
              <span>A weekly series is asked around its day, going by the dates of its chapters. A title without dates is asked every time.</span>
            </div>
            <div class="how">{@render toggle(pref('updates.smart', false), 'Wait for the day a chapter is expected', () => flip('updates.smart'))}</div>
          </div>
          {#if categories.length}
            {#each [['updates.categories', 'Only these categories', 'None picked: all of them.'], ['updates.excluded', 'Never these categories', 'Left out even when picked above.']] as [key, label, hint] (key)}
              <div class="set stack" role="group" aria-label={label}>
                <div class="what"><strong>{label}</strong><span>{hint}</span></div>
                <div class="how">
                  {#each categories as c (c.id)}
                    <button class="btn pill" aria-pressed={pref<number[]>(key, []).includes(c.id)} onclick={() => toggleIn(key, c.id)}>{c.name}</button>
                  {/each}
                </div>
              </div>
            {/each}
          {/if}
        </section>
      {:else if tab.id === 'downloads'}
        <section class="card">
          <h3>Files</h3>
          <div class="set">
            <div class="what"><strong>Delete after reading</strong><span>A chapter's file goes once the chapter is read.</span></div>
            <div class="how">{@render toggle(pref('downloads.deleteAfterReading', false), "Delete a chapter's file once it is read", () => flip('downloads.deleteAfterReading'))}</div>
          </div>
          <div class="set">
            <div class="what"><strong>Download queue</strong><span>What is being saved now, and what waits.</span></div>
            <div class="how"><a class="btn" href="#/downloads"><Icon name="download" size={18} />Open the queue</a></div>
          </div>
          <div class="set">
            <div class="what">
              <strong>Export my files</strong>
              <span>
                Every downloaded chapter in one .zip, a folder for each title. Unzip it into the local folder of Mihon, Suwayomi or Mango to read them
                there.
                {#if files?.chapters}{counted(files.chapters, 'chapter')} of {counted(files.titles, 'title')}, {sized(files.bytes)}.{:else if files}Nothing
                  is downloaded yet.{/if}
              </span>
            </div>
            <div class="how">
              {#if files?.chapters}
                <a class="btn" href="api/export/files" download><Icon name="upload" size={18} />Export files</a>
              {:else}
                <button class="btn" disabled><Icon name="upload" size={18} />Export files</button>
              {/if}
            </div>
          </div>
        </section>
      {:else if tab.id === 'pipeline'}
        {#if pipelineError}
          <div class="set stack"><Notice error={pipelineError} /></div>
        {/if}

        <div class="pipeline-section-bar">
          <div class="pill-group" role="tablist" aria-label="Pipeline sections">
            <button class="pill mini" class:active={pipelineFilter === 'all'} onclick={() => pipelineFilter = 'all'}>
              All Sections
            </button>
            <button class="pill mini" class:active={pipelineFilter === 'paths'} onclick={() => pipelineFilter = 'paths'}>
              Paths & Komga
            </button>
            <button class="pill mini" class:active={pipelineFilter === 'metadata'} onclick={() => pipelineFilter = 'metadata'}>
              Metadata (MangaBaka/KOMF)
            </button>
            <button class="pill mini" class:active={pipelineFilter === 'providers'} onclick={() => pipelineFilter = 'providers'}>
              Providers & TorBox
            </button>
            <button class="pill mini" class:active={pipelineFilter === 'automation'} onclick={() => pipelineFilter = 'automation'}>
              Automation & Profiles
            </button>
            <button class="pill mini" class:active={pipelineFilter === 'maintenance'} onclick={() => pipelineFilter = 'maintenance'}>
              Normalization & Maintenance
            </button>
            <button class="pill mini" class:active={pipelineFilter === 'diagnostics'} onclick={() => pipelineFilter = 'diagnostics'}>
              Hygiene & Diagnostics
            </button>
          </div>
        </div>

        {#if pipelineFilter === 'all' || pipelineFilter === 'paths'}
        <section class="card">
          <h3>Acquisition Pipeline</h3>
            <div class="set">
              <div class="what">
                <strong>Enable Acquisition Pipeline</strong>
                <span>Post-process new volume and chapter acquisitions: enrich metadata, optimize images, publish to Komga library folders, and trigger library scans.</span>
              </div>
              <div class="how">
                {@render toggle(pipeline.enabled, 'Enable pipeline', () => {
                  if (pipeline) pipeline.enabled = !pipeline.enabled;
                })}
              </div>
            </div>
          </section>

          <section class="card">
            <div class="card-header-row">
              <h3>Permanent Library Paths</h3>
              <button class="btn small" onclick={validatePaths} disabled={validatingPaths}>
                <Icon name="check" size={16} />
                {validatingPaths ? 'Validating…' : 'Validate Paths'}
              </button>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Permanent Manga Library</strong>
                <span>Canonical destination folder visible to Mango where standard/English series are published (e.g. <code>/library/manga</code> mounted via Docker).</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="text" bind:value={libraryDirInput} placeholder="/library/manga (container path)" autocomplete="off" />
                </label>
              </div>
              {#if libraryStatus}
                <div class="path-status-row">
                  <span class="status-badge status-{libraryStatus.code}">
                    {#if libraryStatus.code === 'valid'}
                      ✓ Valid / Writable
                    {:else if libraryStatus.code === 'missing'}
                      ✗ Missing inside container
                    {:else if libraryStatus.code === 'not_a_directory'}
                      ✗ Not a directory
                    {:else if libraryStatus.code === 'not_writable'}
                      ✗ Read-only or not writable
                    {:else}
                      Not configured
                    {/if}
                  </span>
                  {#if libraryStatus.code !== 'valid' && libraryStatus.code !== 'not_configured'}
                    <span class="status-msg">{libraryStatus.message}</span>
                  {/if}
                </div>
              {/if}
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Chinese Manga Library</strong>
                <span>Optional separate destination folder visible to Mango for Chinese/Mandarin acquisitions (e.g. <code>/library/manga-chinese</code>).</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="text" bind:value={chineseLibraryDirInput} placeholder="/library/manga-chinese (optional container path)" autocomplete="off" />
                </label>
              </div>
              {#if chineseLibraryStatus}
                <div class="path-status-row">
                  <span class="status-badge status-{chineseLibraryStatus.code}">
                    {#if chineseLibraryStatus.code === 'valid'}
                      ✓ Valid / Writable
                    {:else if chineseLibraryStatus.code === 'missing'}
                      ✗ Missing inside container
                    {:else if chineseLibraryStatus.code === 'not_a_directory'}
                      ✗ Not a directory
                    {:else if chineseLibraryStatus.code === 'not_writable'}
                      ✗ Read-only or not writable
                    {:else}
                      Not configured (optional)
                    {/if}
                  </span>
                  {#if chineseLibraryStatus.code !== 'valid' && chineseLibraryStatus.code !== 'not_configured'}
                    <span class="status-msg">{chineseLibraryStatus.message}</span>
                  {/if}
                </div>
              {/if}
            </div>
          </section>
        {/if}

        {#if pipelineFilter === 'all' || pipelineFilter === 'metadata'}
          <section class="card">
            <h3>MangaBaka Metadata Enrichment</h3>
            <div class="set">
              <div class="what">
                <strong>Enrich Volume Metadata</strong>
                <span>Fetch official release dates, volume summaries, writers, artists, and publishers from MangaBaka.</span>
              </div>
              <div class="how">
                {@render toggle(pipeline.mangabaka_enabled, 'MangaBaka enrichment', () => {
                  if (pipeline) pipeline.mangabaka_enabled = !pipeline.mangabaka_enabled;
                })}
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>MangaBaka API Endpoint</strong>
                <span>Default: https://api.mangabaka.org/v1</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="url" bind:value={mangabakaUrlInput} placeholder="https://api.mangabaka.org/v1" autocomplete="off" />
                </label>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>MangaBaka API Token</strong>
                <span>MangaBaka API key for user library progress sync and enrichment. Leave as "********" to keep existing token.</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="password" bind:value={mangabakaTokenInput} placeholder={pipeline.has_mangabaka_token ? 'Configured (type CLEAR to remove)' : 'Optional API token'} autocomplete="off" />
                </label>
                <button class="btn" onclick={testMangaBaka} disabled={mangabakaTesting}>
                  <Icon name="refresh" size={16} />
                  {mangabakaTesting ? 'Validating…' : 'Validate Connection'}
                </button>
              </div>
            </div>
            {#if mangabakaTestResult}
              <div class="set stack">
                <Notice error={mangabakaTestResult.ok ? null : mangabakaTestResult.message}>
                  {#if mangabakaTestResult.ok}<span>{mangabakaTestResult.message}</span>{/if}
                </Notice>
              </div>
            {/if}
          </section>

          <section class="card">
            <h3>KOMF book metadata</h3>
            <div class="set"><div class="what"><strong>Hand off published English volumes</strong><span>Apply metadata to the exact indexed book. Failures preserve the published book and can be retried separately.</span></div>
              <div class="how">{@render toggle(pipeline.komf_enabled ?? false, 'KOMF handoff', () => { if (pipeline) pipeline.komf_enabled = !pipeline.komf_enabled; })}</div></div>
            <div class="set stack"><div class="what"><strong>KOMF server URL</strong><span>Use an address reachable from the Mango server.</span></div><div class="how wide"><label class="field grow"><input type="url" bind:value={pipeline.komf_url} placeholder="https://komf.example" /></label></div></div>
          </section>

          <!-- OpenTome Physical Release Intelligence Card -->
          <section class="card">
            <div class="card-header-row">
              <div>
                <h3>OpenTome Physical Release Intelligence</h3>
                <span class="muted">Market-specific physical volume release schedules from published OpenTome SQLite catalogues.</span>
              </div>
              {#if openTomeStatus}
                <span
                  class="status-badge"
                  class:status-valid={openTomeStatus.enabled && openTomeStatus.available}
                  class:status-not_configured={!openTomeStatus.enabled}
                  class:status-failed={openTomeStatus.enabled && !openTomeStatus.available}
                >
                  {#if !openTomeStatus.enabled}
                    DISABLED
                  {:else if openTomeStatus.available}
                    AVAILABLE
                  {:else}
                    UNAVAILABLE
                  {/if}
                </span>
              {/if}
            </div>

            <div class="set">
              <div class="what">
                <strong>Enable OpenTome Release Intelligence</strong>
                <span>Evaluate physical volume schedules (Japan/English) for followed titles and notify when volumes release. Strictly read-only; never auto-acquires.</span>
              </div>
              <div class="how">
                {#if openTomeStatus}
                  {@render toggle(openTomeStatus.enabled, 'OpenTome Intelligence', toggleOpenTome)}
                {/if}
              </div>
            </div>

            {#if openTomeStatus}
              <div class="set stack">
                <div class="what">
                  <strong>Catalogue Details</strong>
                  <span>Current local database file and dump metadata.</span>
                </div>
                <div class="how wide">
                  <div class="opentome-status-grid">
                    <div class="ot-meta-item">
                      <span class="ot-k">Dump Version:</span>
                      <strong class="ot-v">{openTomeStatus.version ?? 'Not downloaded'}</strong>
                    </div>
                    <div class="ot-meta-item">
                      <span class="ot-k">Database Size:</span>
                      <strong class="ot-v">
                        {openTomeStatus.fileSize ? (openTomeStatus.fileSize / 1024 / 1024).toFixed(1) + ' MB' : '0 MB'}
                      </strong>
                    </div>
                    <div class="ot-meta-item">
                      <span class="ot-k">Catalogue Path:</span>
                      <code class="ot-v ot-path">{openTomeStatus.cataloguePath}</code>
                    </div>
                    {#if openTomeStatus.lastUpdatedAt}
                      <div class="ot-meta-item">
                        <span class="ot-k">Last Updated:</span>
                        <strong class="ot-v">{ago(openTomeStatus.lastUpdatedAt * 1000)}</strong>
                      </div>
                    {/if}
                    {#if openTomeStatus.lastCheckedAt}
                      <div class="ot-meta-item">
                        <span class="ot-k">Last Checked:</span>
                        <strong class="ot-v">{ago(openTomeStatus.lastCheckedAt * 1000)}</strong>
                      </div>
                    {/if}
                  </div>
                </div>
              </div>

              {#if openTomeStatus.lastError}
                <div class="set stack">
                  <Notice error={openTomeStatus.lastError}>
                    <span>{openTomeStatus.lastError}</span>
                  </Notice>
                </div>
              {/if}

              <div class="set">
                <div class="what">
                  <strong>Update Catalogue</strong>
                  <span>Check for new releases of the OpenTome database manifest (version.json) and stream the verified SQLite archive.</span>
                </div>
                <div class="how opentome-btn-row">
                  <button class="btn" onclick={() => triggerOpenTomeUpdate(false)} disabled={openTomeUpdating}>
                    {#if openTomeUpdating}
                      <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                      Checking…
                    {:else}
                      <Icon name="refresh" size={16} />
                      Check for Updates
                    {/if}
                  </button>
                  <button class="btn" onclick={() => triggerOpenTomeUpdate(true)} disabled={openTomeUpdating} title="Force full re-download regardless of dump date">
                    Force Re-download
                  </button>
                </div>
              </div>
            {/if}
          </section>
        {/if}

        {#if pipelineFilter === 'all' || pipelineFilter === 'paths'}
          <section class="card">
            <h3>Komga Server Integration</h3>
            <div class="set">
              <div class="what">
                <strong>Enable Komga Integration</strong>
                <span>Connect to your Komga instance for ownership discovery and library synchronization.</span>
              </div>
              <div class="how">
                {@render toggle(pipeline.komga_enabled, 'Komga integration', () => {
                  if (pipeline) pipeline.komga_enabled = !pipeline.komga_enabled;
                })}
              </div>
            </div>
            <div class="set">
              <div class="what">
                <strong>Post-Publication Library Scan</strong>
                <span>Automatically trigger a library rescan in Komga when new volumes or chapters are published into the permanent library.</span>
              </div>
              <div class="how">
                {@render toggle(komgaScanEnabledInput, 'Post-publication scan', () => {
                  komgaScanEnabledInput = !komgaScanEnabledInput;
                })}
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Komga Server URL</strong>
                <span>Address of your Komga server. <em>Note for Docker:</em> <code>127.0.0.1</code> inside the container refers to the container itself. Use <code>http://host.docker.internal:25600</code> or your host LAN IP (e.g. <code>http://10.0.0.140:25600</code>) to reach host Komga.</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="url" bind:value={komgaUrlInput} placeholder="http://127.0.0.1:25600" autocomplete="off" />
                </label>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Komga API Key</strong>
                <span>API key generated in Komga under User Settings &gt; API Keys. Read-only validation never scans libraries.</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="password" bind:value={komgaApiKeyInput} placeholder={pipeline.has_komga_api_key ? 'Configured (type CLEAR to remove)' : 'Komga API Key'} autocomplete="off" />
                </label>
                <button class="btn" onclick={testKomga} disabled={komgaTesting}>
                  <Icon name="refresh" size={16} />
                  {komgaTesting ? 'Validating…' : 'Validate Connection'}
                </button>
                <button class="btn" onclick={discoverKomgaLibraries} disabled={discoveringKomgaLibraries}>
                  <Icon name="search" size={16} />
                  {discoveringKomgaLibraries ? 'Discovering…' : 'Discover Libraries'}
                </button>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>English / Default Library</strong>
                <span>Target Komga library for canonical English manga acquisitions. Discovered from Komga or entered manually.</span>
              </div>
              <div class="how wide">
                {#if komgaLibraries.length > 0}
                  <select class="btn grow" bind:value={komgaLibraryIdInput}>
                    <option value="">All libraries (or enter manual ID below)</option>
                    {#if komgaLibraryIdInput && !komgaLibraries.some((l) => l.id === komgaLibraryIdInput)}
                      <option value={komgaLibraryIdInput}>Current / Custom: {komgaLibraryIdInput}</option>
                    {/if}
                    {#each komgaLibraries as lib}
                      <option value={lib.id}>{lib.name} ({lib.id})</option>
                    {/each}
                  </select>
                {/if}
                <label class="field grow">
                  <input type="text" bind:value={komgaLibraryIdInput} placeholder="All libraries or specific Library ID" autocomplete="off" />
                </label>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Chinese Library (Optional)</strong>
                <span>Target Komga library for Chinese-translated manga (Bilibili, etc.).</span>
              </div>
              <div class="how wide">
                {#if komgaLibraries.length > 0}
                  <select class="btn grow" bind:value={komgaChineseLibraryIdInput}>
                    <option value="">Same as default / unassigned</option>
                    {#if komgaChineseLibraryIdInput && !komgaLibraries.some((l) => l.id === komgaChineseLibraryIdInput)}
                      <option value={komgaChineseLibraryIdInput}>Current / Custom: {komgaChineseLibraryIdInput}</option>
                    {/if}
                    {#each komgaLibraries as lib}
                      <option value={lib.id}>{lib.name} ({lib.id})</option>
                    {/each}
                  </select>
                {/if}
                <label class="field grow">
                  <input type="text" bind:value={komgaChineseLibraryIdInput} placeholder="Leave blank to use default, or enter Library ID" autocomplete="off" />
                </label>
              </div>
            </div>
            {#if komgaTestResult}
              <div class="set stack">
                <Notice error={komgaTestResult.ok ? null : komgaTestResult.message}>
                  {#if komgaTestResult.ok}<span>{komgaTestResult.message}</span>{/if}
                </Notice>
              </div>
            {/if}
          </section>

          <section class="card">
            <h3>Image Optimization</h3>
            <div class="set">
              <div class="what">
                <strong>Enable Image Optimization</strong>
                <span>Non-destructively shrink CBZ archives before permanent library publication using RustyLibraryShrinker.</span>
              </div>
              <div class="how">
                {@render toggle(pipeline.optimize_enabled, 'Image optimization', () => {
                  if (pipeline) pipeline.optimize_enabled = !pipeline.optimize_enabled;
                })}
              </div>
            </div>
            <div class="set">
              <div class="what">
                <strong>Target Page Height (px)</strong>
                <span>Maximum/target vertical resolution used during optimization. Default is 2400 px.</span>
              </div>
              <div class="how">
                <label class="field seconds">
                  <span class="sr">Target page height in pixels</span>
                  <input type="number" bind:value={targetHeightInput} min="500" max="10000" step="100" />
                </label>
              </div>
            </div>
            <div class="set">
              <div class="what">
                <strong>Image Quality (1–100)</strong>
                <span>Encoder quality controlling visual fidelity vs file size. Default is 90.</span>
              </div>
              <div class="how">
                <label class="field seconds">
                  <span class="sr">Image quality from 1 to 100</span>
                  <input type="number" bind:value={imageQualityInput} min="1" max="100" />
                </label>
              </div>
            </div>
            <div class="set">
              <div class="what">
                <strong>Minimum Size Savings Required (%)</strong>
                <span>Reject an optimized result if it does not save at least this percentage; preserve/use the original instead. Default is 10%.</span>
              </div>
              <div class="how">
                <label class="field seconds">
                  <span class="sr">Minimum size savings percentage</span>
                  <input type="number" bind:value={minSavingsPercentInput} min="0" max="99" />
                </label>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Optimizer Executable Path (Optional Override)</strong>
                <span>
                  Defaults to the bundled optimizer inside the Mango container (<code>/opt/mango/bin/RustyLibraryShrinker</code>).
                  {#if bundledOptimizerPresent}
                    <strong class="text-spot">Bundled optimizer available.</strong>
                  {/if}
                </span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input
                    type="text"
                    bind:value={optimizerBinInput}
                    placeholder={resolvedOptimizerBin ? `${resolvedOptimizerBin} (detected)` : '/opt/mango/bin/RustyLibraryShrinker (bundled default)'}
                    autocomplete="off"
                  />
                </label>
              </div>
              {#if optimizerStatus}
                <div class="path-status-row">
                  <span class="status-badge status-{optimizerStatus.code}">
                    {#if optimizerStatus.code === 'valid'}
                      ✓ Operational ({optimizerStatus.version ?? 'Executable'} — {optimizerStatus.source === 'bundled' ? 'bundled' : optimizerStatus.source === 'path' ? 'system PATH' : 'custom override'}{optimizerStatus.detected_architecture ? ` [${optimizerStatus.detected_architecture}]` : ''})
                    {:else if optimizerStatus.code === 'missing'}
                      ✗ Not found ({optimizerStatus.source === 'custom' ? 'custom path' : 'bundled optimizer'})
                    {:else if optimizerStatus.code === 'not_a_file'}
                      ✗ Not a file
                    {:else if optimizerStatus.code === 'not_executable'}
                      ✗ Not executable by Mango runtime
                    {:else if optimizerStatus.code === 'failed'}
                      ✗ Invocation test failed
                    {:else}
                      Not configured
                    {/if}
                  </span>
                  {#if optimizerStatus.code === 'valid'}
                    <span class="status-msg muted">Architecture: {optimizerStatus.detected_architecture ?? 'unknown'} • Verified operational: {optimizerStatus.is_operational ? 'Yes' : 'No'}</span>
                  {:else if optimizerStatus.message}
                    <span class="status-msg">{optimizerStatus.message}</span>
                  {/if}
                </div>
              {/if}
            </div>
          </section>
        {/if}

        {#if pipelineFilter === 'all' || pipelineFilter === 'providers'}
          <section class="card">
            <h3>TorBox Download Backend</h3>
            <div class="set">
              <div class="what">
                <strong>Enable TorBox Backend</strong>
                <span>Use TorBox cloud torrent engine to acquire releases discovered from Nyaa and feed them into the native pipeline.</span>
              </div>
              <div class="how">
                {@render toggle(torboxEnabledInput, 'TorBox backend', () => {
                  torboxEnabledInput = !torboxEnabledInput;
                })}
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>TorBox API Endpoint</strong>
                <span>Default: https://api.torbox.app/v1/api</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="url" bind:value={torboxUrlInput} placeholder="https://api.torbox.app/v1/api" autocomplete="off" />
                </label>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>TorBox API Token</strong>
                <span>Bearer token from your TorBox dashboard. Never exposed in responses or UI. Type CLEAR to remove.</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input
                    type="password"
                    bind:value={torboxTokenInput}
                    placeholder={acquisition.hasTorboxToken ? 'Configured (type CLEAR to remove)' : 'TorBox API Token'}
                    autocomplete="off"
                  />
                </label>
                <button class="btn" onclick={testTorBox} disabled={torboxTesting}>
                  <Icon name="refresh" size={16} />
                  {torboxTesting ? 'Validating…' : 'Validate Connection'}
                </button>
              </div>
            </div>
            {#if torboxTestResult}
              <div class="set stack">
                <Notice error={torboxTestResult.ok ? null : torboxTestResult.message}>
                  {#if torboxTestResult.ok}<span>{torboxTestResult.message}</span>{/if}
                </Notice>
              </div>
            {:else if acquisition.torboxStatusMessage}
              <div class="path-status-row" style="margin-top: 0.5rem;">
                <span class="status-badge status-{acquisition.torboxValidated ? 'valid' : 'failed'}">
                  {acquisition.torboxValidated ? '✓ Connected' : '✗ Error'}: {acquisition.torboxStatusMessage}
                </span>
              </div>
            {/if}
          </section>

          <section class="card">
            <h3>Nyaa Release Discovery</h3>
            <div class="set">
              <div class="what">
                <strong>Enable Nyaa Discovery</strong>
                <span>Discover canonical manga releases from Nyaa directly inside title overview pages.</span>
              </div>
              <div class="how">
                {@render toggle(nyaaEnabledInput, 'Nyaa discovery', () => {
                  nyaaEnabledInput = !nyaaEnabledInput;
                })}
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Nyaa Base URL</strong>
                <span>Default: https://nyaa.si</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="url" bind:value={nyaaUrlInput} placeholder="https://nyaa.si" autocomplete="off" />
                </label>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Nyaa Category</strong>
                <span>Literature - English-translated category filter code. Default: <code>3_1</code>.</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="text" bind:value={nyaaCategoryInput} placeholder="3_1" autocomplete="off" />
                </label>
              </div>
            </div>
          </section>

          <section class="card">
            <h3>Madokami Direct Acquisition Provider</h3>
            <span class="muted">
              First-class direct HTTP download provider for discrete manga volumes with safe Range resume.
            </span>

            <div class="set">
              <div class="what">
                <strong>Enable Madokami Provider</strong>
                <span>Discover and acquire discrete manga volumes directly via HTTP from Madokami.</span>
              </div>
              <div class="how">
                {@render toggle(madokamiEnabledInput, 'Madokami provider', () => {
                  madokamiEnabledInput = !madokamiEnabledInput;
                })}
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>Madokami Base URL</strong>
                <span>Default: https://manga.madokami.al</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="url" bind:value={madokamiUrlInput} placeholder="https://manga.madokami.al" autocomplete="off" />
                </label>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>HTTP Basic Auth Username</strong>
                <span>Username for Madokami account.</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="text" bind:value={madokamiUsernameInput} placeholder="Username" autocomplete="off" />
                </label>
              </div>
            </div>
            <div class="set stack">
              <div class="what">
                <strong>HTTP Basic Auth Password</strong>
                <span>Password for Madokami account.</span>
              </div>
              <div class="how wide">
                <label class="field grow">
                  <input type="password" bind:value={madokamiPasswordInput} placeholder="Password" autocomplete="off" />
                </label>
              </div>
            </div>
          </section>
        {/if}

        {#if pipelineFilter === 'all' || pipelineFilter === 'automation'}
          <section class="card">
            <h3>Acquisition Operating Mode & Automation</h3>
            <span class="muted">
              Configure system-wide acquisition safety, simulation behavior, and publication automation.
            </span>

            <div class="set">
              <div class="what">
                <strong>Operating Mode</strong>
                <span>
                  <strong>Simulation Mode:</strong> All download requests generate simulated previews without network activity, remote API jobs, or storage modifications.<br />
                  <strong>Normal Mode:</strong> Explicit user-initiated downloads retrieve real archives from TorBox or Madokami after validation.
                </span>
              </div>
              <div class="how">
                <div class="seg" role="group" aria-label="Acquisition Operating Mode">
                  <button
                    aria-pressed={operatingModeInput === 'simulation'}
                    onclick={() => { operatingModeInput = 'simulation'; }}
                  >Simulation Mode</button>
                  <button
                    aria-pressed={operatingModeInput === 'normal'}
                    onclick={() => toggleOperatingMode('normal')}
                  >Normal Mode</button>
                </div>
              </div>
            </div>

            <div class="set">
              <div class="what">
                <strong>Automatic Safe Publication</strong>
                <span>
                  When enabled, validated exact-volume manual acquisitions automatically publish into Komga library folders. When disabled, acquisitions remain safely staged until separate publication approval.
                </span>
              </div>
              <div class="how">
                {@render toggle(autoPublishManualInput, 'Auto-publish manual acquisitions', () => {
                  autoPublishManualInput = !autoPublishManualInput;
                })}
              </div>
            </div>

            <div class="set">
              <div class="what">
                <strong>Global Automatic Acquisition Master Switch</strong>
                <span>
                  Master kill switch. When disabled, background automated acquisition is completely halted across all titles.
                </span>
              </div>
              <div class="how">
                {@render toggle(autoAcquireSettings.enabled, 'Global auto-acquire', toggleAutoAcquireGlobal)}
              </div>
            </div>

            <div class="set">
              <div class="what">
                <strong>Automation Operating Policy</strong>
                <span>
                  <strong>Mode A — Monitor Only (Default):</strong> Monitor followed titles, discover releases, score candidates, and record availability without queueing or downloading.<br />
                  <strong>Mode B — Review Before Acquisition:</strong> Propose discovered releases into the Acquisition Review Queue for operator confirmation.<br />
                  <strong>Mode C — Automatic Acquisition:</strong> Opt-in autonomous queueing of high-confidence, verified English releases (requires master switch ON).
                </span>
              </div>
              <div class="how">
                <div class="seg" role="group" aria-label="Automation Policy Mode">
                  <button
                    aria-pressed={monitoringSettings.automationMode === 'monitor_only'}
                    onclick={() => updateAutomationMode('monitor_only')}
                  >Mode A (Monitor Only)</button>
                  <button
                    aria-pressed={monitoringSettings.automationMode === 'review_before_acquisition'}
                    onclick={() => updateAutomationMode('review_before_acquisition')}
                  >Mode B (Review Queue)</button>
                  <button
                    aria-pressed={monitoringSettings.automationMode === 'automatic_acquisition'}
                    onclick={() => updateAutomationMode('automatic_acquisition')}
                  >Mode C (Auto-Acquire)</button>
                </div>
              </div>
            </div>

            <div class="set">
              <div class="what">
                <strong>Monitored Followed Lists</strong>
                <span>
                  Select which personal tracking lists are monitored for missing volumes and release discovery.
                </span>
              </div>
              <div class="how" style="flex-direction: column; align-items: flex-start; gap: 8px;">
                <div class="pill-group tracked-statuses-group" role="group" aria-label="Monitored tracking lists">
                  {#each ['reading', 'planning', 'completed', 'paused', 'dropped'] as status}
                    {@const isSelected = monitoringSettings.trackedStatuses?.includes(status) ?? false}
                    <button
                      type="button"
                      class="tracked-status-btn"
                      class:selected={isSelected}
                      aria-pressed={isSelected}
                      disabled={savingTrackedStatuses}
                      onclick={() => toggleTrackedStatus(status)}
                    >
                      {#if isSelected}
                        <Icon name="check" size={13} stroke={2.5} />
                      {/if}
                      <span>{status.charAt(0).toUpperCase() + status.slice(1)}</span>
                    </button>
                  {/each}
                </div>
                <div class="tracked-summary">
                  <span>Monitoring: <strong>{monitoredSummaryText}</strong></span>
                  {#if savingTrackedStatuses}
                    <span class="saving-indicator">
                      <span class="spin"><Icon name="spinner" size={13} stroke={2.5} /></span>
                      Saving…
                    </span>
                  {/if}
                </div>
              </div>
            </div>

            <div class="set">
              <div class="what">
                <strong>Monitoring Check Interval</strong>
                <span>
                  Frequency in hours for evaluating followed titles, missing content, and candidate releases.
                </span>
              </div>
              <div class="how">
                <label class="field" style="width: 140px;">
                  <input
                    type="number"
                    min="1"
                    max="168"
                    value={monitoringSettings.intervalHours}
                    onchange={(e) => updateMonitoringInterval(parseInt(e.currentTarget.value) || 12)}
                  />
                  <span class="muted font-small">hours</span>
                </label>
              </div>
            </div>

            <div class="path-status-row" style="margin-top: 0.75rem; display: flex; flex-wrap: wrap; gap: 0.5rem;">
              <span class="status-badge status-{monitoringSettings.automationMode === 'monitor_only' ? 'valid' : monitoringSettings.automationMode === 'review_before_acquisition' ? 'not_configured' : 'failed'}">
                Policy: {monitoringSettings.automationMode === 'monitor_only' ? 'Mode A (Monitor Only)' : monitoringSettings.automationMode === 'review_before_acquisition' ? 'Mode B (Review Before Acquisition)' : 'Mode C (Automatic Acquisition)'}
              </span>
              <span class="status-badge status-{operatingModeInput === 'simulation' ? 'not_configured' : 'valid'}">
                {#if operatingModeInput === 'simulation'}
                  Mode: Simulation (Protected)
                {:else}
                  Mode: Normal (Live Downloads Active)
                {/if}
              </span>
              <span class="status-badge status-{autoPublishManualInput ? 'valid' : 'not_configured'}">
                {#if autoPublishManualInput}
                  Publication: Auto-Publish
                {:else}
                  Publication: Staging Only (Approval Required)
                {/if}
              </span>
              <span class="status-badge status-{autoAcquireSettings.enabled ? 'valid' : 'failed'}">
                {#if autoAcquireSettings.enabled}
                  Auto-Acquisition: Master Switch ON
                {:else}
                  Auto-Acquisition: Master Switch OFF
                {/if}
              </span>
            </div>
          </section>

          <section class="card">
            <div class="card-header-flex">
              <div>
                <h3>Acquisition Profiles</h3>
                <span class="muted">Define preferences, release types, quality standards, and budget constraints for automated missing-content planning.</span>
              </div>
              <button class="btn pill icon-text" onclick={startNewProfile}>
                <Icon name="plus" size={16} /> New Profile
              </button>
            </div>

            {#if profilesLoading}
              <p class="muted">Loading acquisition profiles…</p>
            {:else if profiles.length === 0}
              <p class="muted">No profiles found.</p>
            {:else}
              <div class="profiles-grid">
                {#each profiles as prof (prof.id)}
                  <div class="profile-card" class:default-card={prof.isDefault}>
                    <div class="profile-card-header">
                      <div>
                        <strong>{prof.name}</strong>
                        {#if prof.isDefault}
                          <span class="tag spot" style="margin-left: 0.5rem;">DEFAULT</span>
                        {/if}
                      </div>
                      <div class="profile-actions">
                        <button class="btn pill icon-text" onclick={() => startEditProfile(prof)}>
                          <Icon name="pencil" size={14} /> Edit
                        </button>
                        <button class="btn pill icon-text" onclick={() => handleDuplicateProfile(prof.id)}>
                          <Icon name="copy" size={14} /> Duplicate
                        </button>
                        {#if !prof.isDefault}
                          <button class="btn pill" onclick={() => handleSetDefaultProfile(prof)} title="Make Default">
                            Set Default
                          </button>
                          <button class="btn pill danger icon" onclick={() => handleDeleteProfile(prof)} title="Delete Profile">
                            <Icon name="trash" size={14} />
                          </button>
                        {/if}
                      </div>
                    </div>

                    <div class="profile-attrs-table">
                      <div class="attr-row">
                        <span class="attr-k">Language</span>
                        <span class="attr-v">{prof.language === 'en' ? 'English' : prof.language === 'zh' ? 'Chinese' : prof.language}</span>
                      </div>
                      <div class="attr-row">
                        <span class="attr-k">Release Preference</span>
                        <span class="attr-v capitalize">{prof.preferredReleaseType} releases</span>
                      </div>
                      <div class="attr-row">
                        <span class="attr-k">Source Selection</span>
                        <span class="attr-v capitalize">{prof.sourceSelection}</span>
                      </div>
                      <div class="attr-row">
                        <span class="attr-k">Minimum Seeders</span>
                        <span class="attr-v">{prof.minSeeders}</span>
                      </div>
                      <div class="attr-row">
                        <span class="attr-k">Prefer Digital</span>
                        <span class="attr-v">{prof.preferDigital ? 'Yes' : 'No'}</span>
                      </div>
                      <div class="attr-row">
                        <span class="attr-k">Trusted Only</span>
                        <span class="attr-v">{prof.trustedOnly ? 'Yes' : 'No'}</span>
                      </div>
                      <div class="attr-row">
                        <span class="attr-k">Collection Policy</span>
                        <span class="attr-v capitalize">{prof.collectionPolicy.replace('_', ' ')}</span>
                      </div>
                      <div class="attr-row">
                        <span class="attr-k">Auto-acquire</span>
                        <span class="attr-v"><span class="tag">OFF (Disabled)</span></span>
                      </div>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </section>
        {/if}

        {#if pipelineFilter === 'all' || pipelineFilter === 'providers'}
          <section class="card">
            <h3>Acquisition Sources & Health</h3>
            <span class="muted">Operational health, discovery capabilities, and uploader reliability across configured acquisition providers.</span>

            <div class="sources-health-list">
              {#each sourceHealthReports as src (src.sourceId)}
                <div class="source-health-card">
                  <div class="source-health-header">
                    <strong>{src.name}</strong>
                    <span class="status-badge status-{src.status === 'healthy' ? 'valid' : src.status === 'degraded' ? 'not_configured' : 'failed'}">
                      {src.status.toUpperCase()}
                    </span>
                  </div>
                  <div class="source-health-meta">
                    <span>Volumes: <strong>{src.supportsVolumes ? 'Yes' : 'No'}</strong></span>
                    <span>Collections: <strong>{src.supportsCollections ? 'Yes' : 'No'}</strong></span>
                    <span>Chapters: <strong>{src.supportsChapters ? 'Yes' : 'No'}</strong></span>
                    <span>Backend: <strong>{src.requiresExternalBackend ? 'TorBox External' : 'Internal Reader'}</strong></span>
                    {#if src.lastResponseTimeMs}
                      <span>Avg Ping: <strong>{src.lastResponseTimeMs}ms</strong></span>
                    {/if}
                    {#if src.lastSuccessAt}
                      <span>Last Success: <strong>{ago(src.lastSuccessAt * 1000)}</strong></span>
                    {/if}
                  </div>
                  {#if src.lastError}
                    <div class="event-error" style="margin-top: 0.5rem;">{src.lastError}</div>
                  {/if}
                </div>
              {/each}
            </div>
          </section>
        {/if}

        {#if pipelineFilter === 'all' || pipelineFilter === 'paths' || pipelineFilter === 'metadata' || pipelineFilter === 'providers' || pipelineFilter === 'automation'}
          <div class="action-bar">
            <button class="btn spot" onclick={savePipeline} disabled={pipelineSaving}>
              <Icon name="check" size={18} />
              {pipelineSaving ? 'Saving…' : 'Save Pipeline Settings'}
            </button>
            <button class="btn" onclick={loadPipeline} disabled={pipelineLoading}>
              <Icon name="refresh" size={18} />
              Refresh
            </button>
          </div>
        {/if}

        {#if pipelineFilter === 'all' || pipelineFilter === 'maintenance'}
          <!-- Library Metadata Normalization & Maintenance -->
          <section class="card">
            <div class="card-header-row">
              <div>
                <h3>Library Metadata Normalization & Maintenance</h3>
                <span class="muted">Safe, dry-run-first metadata inspection and atomic ComicInfo.xml normalization for existing library CBZs</span>
              </div>
            </div>

            <div class="set stack">
              <div class="what">
                <strong>Select Komga Library</strong>
                <span>Choose a library for DRY RUN. Comics and Chinese libraries are skipped; only identifiable English Manga libraries are eligible.</span>
              </div>
              <div class="how wide">
                <div class="library-select-row">
                  <select class="field grow" bind:value={maintenanceLibraryId}>
                    <option value="" disabled>Select a library…</option>
                    {#if komgaLibraries.length > 0}
                      {#each komgaLibraries as lib}
                        <option value={lib.id}>
                          {lib.name} ({lib.root}) — policy checked by server
                        </option>
                      {/each}
                    {:else}
                      <option value="" disabled>Discover Komga libraries in Pipeline settings first</option>
                    {/if}
                  </select>
                  <button
                    class="btn spot"
                    onclick={handleRunDryRun}
                    disabled={!maintenanceLibraryId || maintenanceAnalyzing || maintenanceNormalizing}
                  >
                    <Icon name={maintenanceAnalyzing ? 'spinner' : 'search'} size={16} />
                    {maintenanceAnalyzing ? 'Analyzing…' : 'Run Dry-Run Analysis'}
                  </button>
                </div>
              </div>
            </div>

            {#if maintenanceError}
              <div class="set stack">
                <Notice error={maintenanceError} />
              </div>
            {/if}

            {#if analysisSummary}
              <p class="muted">DRY RUN — {analysisSummary.totalBooks} books considered. No canonical files changed. Review per-book changes before approving mutation.</p>
              <!-- Metric Badges / Summary Cards -->
              <div class="analysis-summary-grid">
                <div class="metric-card metric-matched">
                  <span class="metric-val">{analysisSummary.matchedCount}</span>
                  <span class="metric-lbl">Matched Books</span>
                </div>
                <div class="metric-card metric-already">
                  <span class="metric-val">{analysisSummary.alreadyNormalizedCount}</span>
                  <span class="metric-lbl">Already Normalized</span>
                </div>
                <div class="metric-card metric-update">
                  <span class="metric-val">{analysisSummary.needsUpdateCount}</span>
                  <span class="metric-lbl">Needs Update</span>
                </div>
                <div class="metric-card metric-ambiguous">
                  <span class="metric-val">{analysisSummary.ambiguousCount}</span>
                  <span class="metric-lbl">Ambiguous</span>
                </div>
                <div class="metric-card metric-unmatched">
                  <span class="metric-val">{analysisSummary.unmatchedCount}</span>
                  <span class="metric-lbl">Unmatched</span>
                </div>
                <div class="metric-card metric-skipped">
                  <span class="metric-val">{analysisSummary.skippedCount}</span>
                  <span class="metric-lbl">Skipped (Not Applicable)</span>
                </div>
                <div class="metric-card metric-error">
                  <span class="metric-val">{analysisSummary.errorCount}</span>
                  <span class="metric-lbl">Errors</span>
                </div>
              </div>

              <!-- Batch Action Buttons -->
              <div class="maintenance-actions-bar">
                <div class="left-actions">
                  <button
                    class="btn ink"
                    onclick={handleNormalizeSelected}
                    disabled={selectedBookKeys.size === 0 || selectedBookKeys.size > 25 || maintenanceNormalizing || maintenanceAnalyzing}
                  >
                    <Icon name={maintenanceNormalizing ? 'spinner' : 'check'} size={16} />
                    Normalize Selected ({selectedBookKeys.size})
                  </button>
                  <button
                    class="btn ink"
                    onclick={handleNormalizeBatch25}
                    disabled={analysisSummary.needsUpdateCount === 0 || maintenanceNormalizing || maintenanceAnalyzing}
                  >
                    Normalize Batch of 25
                  </button>
                </div>
                <span class="muted font-small">
                  REAL MUTATION: replaces canonical CBZ metadata, up to 25 books per batch. Verified full-file backups are retained for rollback.
                </span>
              </div>

              <!-- Results Tree / Table -->
              <div class="normalization-table-wrap">
                {#each analysisSummary.series as series}
                  <div class="series-norm-card">
                    <div class="series-norm-header">
                      <div class="series-norm-title">
                        <strong>{series.seriesTitle}</strong>
                        <span class="status-badge status-{series.status.toLowerCase()}">
                          {series.status}
                        </span>
                        {#if series.mangabakaId}
                          <span class="pill-badge active">MB #{series.mangabakaId}</span>
                        {/if}
                      </div>
                      <span class="series-stats">
                        {series.books.length} books ({series.needsUpdateCount} needs update, {series.alreadyNormalizedCount} current)
                      </span>
                    </div>

                    <p class="muted font-small">{series.reason}</p>
                    {#if series.books.length > 0}
                      <div class="books-norm-list">
                        {#each series.books as book}
                          {@const bookKey = `${series.komgaSeriesId}:${book.komgaBookId}`}
                          <div class="book-norm-row" class:needs-update={book.status === 'NEEDS_UPDATE'}>
                            <div class="book-row-left">
                              {#if book.status === 'NEEDS_UPDATE'}
                                <input
                                  type="checkbox"
                                  checked={selectedBookKeys.has(bookKey)}
                                  onchange={(e) => {
                                    const next = new Set(selectedBookKeys);
                                    if (e.currentTarget.checked) next.add(bookKey);
                                    else next.delete(bookKey);
                                    selectedBookKeys = next;
                                  }}
                                />
                              {:else}
                                <span class="checkbox-spacer"></span>
                              {/if}
                              <span class="book-name" title={book.canonicalPath}>
                                {book.bookName}
                                {#if book.volume !== null && book.volume !== undefined}
                                  (Vol {book.volume})
                                {/if}
                              </span>
                            </div>

                            <div class="book-row-right">
                              <span class="muted font-small">{book.reason}</span>
                              <span class="status-badge status-{book.status.toLowerCase()}">
                                {book.status}
                              </span>
                              {#if book.diffs.length > 0}
                                <details class="diff-badge"><summary>{book.diffs.length} changes</summary>
                                    {#each book.diffs as diff}
                                      <div>{diff.field}: {diff.current ?? '∅'} → {diff.proposed ?? '∅'}</div>
                                    {/each}
                                </details>
                              {/if}
                            </div>
                          </div>
                        {/each}
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}

            <!-- Normalization Journal & Rollback History -->
            <div class="history-section">
              <div class="history-header">
                <h4>Normalization Journal & Rollback</h4>
                <button class="btn icon small" onclick={loadNormalizationHistory} disabled={historyLoading} title="Refresh history">
                  <Icon name="refresh" size={14} />
                </button>
              </div>

              {#if normalizationHistory.length === 0}
                <p class="muted font-small">No library normalization executions recorded yet.</p>
              {:else}
                <div class="history-list">
                  {#each normalizationHistory as rec (rec.id)}
                    <div class="history-item">
                      <div class="hist-top">
                        <span class="hist-series"><strong>{rec.seriesTitle}</strong> (Vol {rec.volume})</span>
                        <span class="status-badge status-{rec.status}">
                          {rec.status.toUpperCase()}
                        </span>
                        <span class="hist-time">{ago(rec.normalizedAt * 1000)}</span>
                      </div>
                      <div class="hist-path monospace font-small">{rec.canonicalPath}</div>
                      {#if rec.errorMessage}<p class="muted font-small">{rec.errorMessage}</p>{/if}
                      {#if rec.refreshPending}
                        <button class="btn small" onclick={async () => {
                          try { await retryNormalizationRefresh(rec.id); await loadNormalizationHistory(); }
                          catch (e) { toast.error(message(e)); }
                        }}>Retry Komga metadata refresh</button>
                      {/if}
                      {#if rec.status === 'normalized' && rec.rollbackAvailable}
                        <div class="hist-actions">
                          <button
                            class="btn small"
                            onclick={() => handleRollbackRecord(rec.id)}
                            disabled={maintenanceRollingBack === rec.id}
                          >
                            <Icon name="refresh" size={14} />
                            {maintenanceRollingBack === rec.id ? 'Reverting…' : 'Revert to Original'}
                          </button>
                        </div>
                      {:else if rec.status === 'normalized'}
                        <p class="muted font-small">Legacy history: no verified full-file backup; automatic rollback unavailable.</p>
                      {:else if rec.status === 'reverted'}
                        <div class="hist-reverted-note font-small muted">
                          ✓ Reverted {rec.revertedAt ? ago(rec.revertedAt * 1000) : ''}
                        </div>
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          </section>

          <!-- Library-Wide Manga Naming Standardization & Audit -->
          <section class="card">
            <div class="card-header-row">
              <div>
                <h3>Library Naming Standardization & Audit</h3>
                <span class="muted">Read-only audit and approved metadata correction for canonical book titles (Volume 01, Volume 02, etc.)</span>
              </div>
            </div>

            <div class="set stack">
              <div class="what">
                <strong>Select Komga Library</strong>
                <span>Audit existing Komga titles against fork canonical standards without modifying files.</span>
              </div>
              <div class="how wide">
                <div class="library-select-row">
                  <select class="field grow" bind:value={namingLibraryId}>
                    <option value="">All Configured Manga Libraries</option>
                    {#if komgaLibraries.length > 0}
                      {#each komgaLibraries as lib}
                        <option value={lib.id}>{lib.name} ({lib.root})</option>
                      {/each}
                    {/if}
                  </select>
                  <button
                    class="btn spot"
                    onclick={handleRunNamingAudit}
                    disabled={namingAuditing || namingApplying}
                  >
                    <Icon name={namingAuditing ? 'spinner' : 'search'} size={16} />
                    {namingAuditing ? 'Auditing…' : 'Run Naming Audit'}
                  </button>
                </div>
              </div>
            </div>

            {#if namingError}
              <div class="set stack">
                <Notice error={namingError} />
              </div>
            {/if}

            {#if namingReport}
              <p class="muted">READ-ONLY AUDIT — {namingReport.totalBooks} books across {namingReport.totalSeries} series inspected. No files modified.</p>
              
              <!-- Metric Badges -->
              <div class="analysis-summary-grid">
                <div class="summary-metric-card">
                  <div class="metric-value">{namingReport.totalBooks}</div>
                  <div class="metric-label">Total Books</div>
                </div>
                <div class="summary-metric-card highlight-green">
                  <div class="metric-value">{namingReport.canonicalCount}</div>
                  <div class="metric-label">Already Canonical</div>
                </div>
                <div class="summary-metric-card highlight-amber">
                  <div class="metric-value">{namingReport.inconsistentTitleCount}</div>
                  <div class="metric-label">Inconsistent Titles</div>
                </div>
                <div class="summary-metric-card highlight-amber">
                  <div class="metric-value">{namingReport.inconsistentNumberCount}</div>
                  <div class="metric-label">Inconsistent Numbers</div>
                </div>
                <div class="summary-metric-card highlight-blue">
                  <div class="metric-value">{namingReport.safeCorrectionCount}</div>
                  <div class="metric-label">Safe to Standardize</div>
                </div>
                {#if namingReport.duplicateNumberCount > 0}
                  <div class="summary-metric-card highlight-red">
                    <div class="metric-value">{namingReport.duplicateNumberCount}</div>
                    <div class="metric-label">Duplicate Numbers</div>
                  </div>
                {/if}
              </div>

              <!-- Action Toolbar -->
              <div class="normalization-action-bar">
                <div class="action-bar-left">
                  <span class="selection-count">
                    <strong>{selectedNamingBookIds.size}</strong> of {namingReport.items.length} books selected
                  </span>
                  <label class="checkbox-label" style="margin-left: 1rem; display: inline-flex; align-items: center; gap: 0.5rem;">
                    <input type="checkbox" bind:checked={namingApplyDryRun} />
                    <span>Dry run only</span>
                  </label>
                  <label class="checkbox-label" style="margin-left: 1rem; display: inline-flex; align-items: center; gap: 0.5rem;">
                    <input type="checkbox" bind:checked={namingUpdateCbz} />
                    <span>Update CBZ ComicInfo inside files</span>
                  </label>
                </div>
                <div class="action-bar-right">
                  <button
                    class="btn spot"
                    onclick={handleApplyNamingCorrections}
                    disabled={namingApplying || selectedNamingBookIds.size === 0}
                  >
                    <Icon name={namingApplying ? 'spinner' : 'check'} size={16} />
                    {namingApplying ? 'Applying…' : (namingApplyDryRun ? 'Dry-Run Selected' : 'Apply Approved Corrections')}
                  </button>
                </div>
              </div>

              <!-- Audit Preview Table -->
              <div class="table-container" style="max-height: 400px; overflow-y: auto; margin-top: 1rem;">
                <table class="table striped font-small" style="width: 100%;">
                  <thead>
                    <tr>
                      <th style="width: 40px;">
                        <input
                          type="checkbox"
                          checked={selectedNamingBookIds.size > 0 && selectedNamingBookIds.size === (namingReport?.items.filter(i => i.isSafe && !i.isAlreadyCanonical).length ?? 0)}
                          onchange={(e) => {
                            if ((e.target as HTMLInputElement).checked && namingReport) {
                              selectedNamingBookIds = new Set(namingReport.items.filter(i => i.isSafe && !i.isAlreadyCanonical).map(i => i.bookId));
                            } else {
                              selectedNamingBookIds = new Set();
                            }
                          }}
                        />
                      </th>
                      <th>Series</th>
                      <th>Book ID</th>
                      <th>Existing Title</th>
                      <th>Proposed Title</th>
                      <th>Number</th>
                      <th>Status</th>
                      <th>Confidence</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each namingReport.items as item}
                      <tr>
                        <td>
                          <input
                            type="checkbox"
                            checked={selectedNamingBookIds.has(item.bookId)}
                            disabled={!item.isSafe || item.isAlreadyCanonical}
                            onchange={(e) => {
                              const checked = (e.target as HTMLInputElement).checked;
                              const next = new Set(selectedNamingBookIds);
                              if (checked) next.add(item.bookId);
                              else next.delete(item.bookId);
                              selectedNamingBookIds = next;
                            }}
                          />
                        </td>
                        <td><strong>{item.seriesTitle}</strong></td>
                        <td><code class="font-small">{item.bookId}</code></td>
                        <td><span class="muted">{item.existingTitle}</span></td>
                        <td>
                          <strong style="color: var(--theme-accent, #3b82f6);">{item.proposedTitle}</strong>
                        </td>
                        <td>{item.proposedNumber}</td>
                        <td>
                          {#if item.isAlreadyCanonical}
                            <span class="badge green">Canonical</span>
                          {:else if item.conflictType}
                            <span class="badge red">{item.conflictType}</span>
                          {:else if item.conformity === 'INCONSISTENT_TITLE'}
                            <span class="badge amber">Inconsistent Title</span>
                          {:else}
                            <span class="badge">{item.conformity}</span>
                          {/if}
                        </td>
                        <td>
                          <span class="badge {item.confidence === 'HIGH' ? 'green' : (item.confidence === 'MEDIUM' ? 'amber' : 'red')}">
                            {item.confidence}
                          </span>
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}

            <!-- Naming Correction Backups / Rollback Section -->
            <div class="set stack" style="margin-top: 1.5rem;">
              <div class="what">
                <strong>Metadata Correction History & Rollback</strong>
                <span>Durable audit backups of all applied Komga metadata corrections. Roll back anytime to restore original titles.</span>
              </div>
              <div class="how wide">
                {#if namingBackupsLoading}
                  <div class="muted">Loading backups…</div>
                {:else if namingBackups.length === 0}
                  <div class="muted">No metadata corrections have been applied yet.</div>
                {:else}
                  <div class="history-list" style="margin-top: 0.5rem;">
                    {#each namingBackups.slice(0, 10) as b}
                      <div class="history-item-row" style="display: flex; justify-content: space-between; align-items: center; padding: 0.5rem 0; border-bottom: 1px solid var(--border-color, #333);">
                        <div>
                          <strong>{b.seriesTitle}</strong>: 
                          <span class="muted">{b.originalTitle || '(none)'}</span> &rarr; <strong>{b.newTitle}</strong>
                          <div class="font-small muted">
                            Batch: <code>{b.backupId}</code> &middot; Status: <span class="badge {b.status === 'verified' ? 'green' : (b.status === 'rolled_back' ? 'muted' : 'amber')}">{b.status}</span> &middot; {ago(b.createdAt * 1000)}
                          </div>
                        </div>
                        {#if b.status === 'applied' || b.status === 'verified'}
                          <button
                            class="btn small"
                            onclick={() => handleRollbackNaming(b.backupId)}
                            disabled={namingRollingBack === b.backupId}
                          >
                            <Icon name="refresh" size={14} />
                            {namingRollingBack === b.backupId ? 'Reverting…' : 'Rollback Batch'}
                          </button>
                        {:else if b.status === 'rolled_back'}
                          <span class="font-small muted">✓ Rolled back {b.rolledBackAt ? ago(b.rolledBackAt * 1000) : ''}</span>
                        {/if}
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
          </section>

          <section class="card">
            <div class="card-header-row">
              <div>
                <h3>Library Integrity & Diagnostics Audit</h3>
                <p class="muted font-small">Comprehensive sanity audit across tracker mappings, MangaBaka identities, Komga physical ownership, duplicate records, outbox queue, and auto-acquire states.</p>
              </div>
              <button class="btn small" onclick={runIntegrityAudit} disabled={integrityLoading}>
                <Icon name="refresh" size={16} />
                {integrityLoading ? 'Auditing…' : 'Run Audit'}
              </button>
            </div>

            {#if !integrityReport && !integrityLoading}
              <div class="empty-state">
                Click "Run Audit" to inspect library mappings, metadata consistency, and queue integrity.
              </div>
            {:else if integrityReport}
              {@const isHealthy = integrityReport.status === 'healthy' || integrityReport.totalIssues === 0}
              <div class="integrity-summary-banner {isHealthy ? 'banner-healthy' : 'banner-warning'}">
                <div class="banner-main">
                  <strong>{isHealthy ? '✓ Library Integrity Healthy' : `⚠️ ${integrityReport.totalIssues} Integrity Anomalies Detected`}</strong>
                  <span>Audited {ago(integrityReport.checkedAt * 1000)}</span>
                </div>
              </div>

              <div class="integrity-metrics-grid">
                <div class="metric-box">
                  <span class="metric-val">{integrityReport.issues.filter(i => i.category === 'orphan_tracker_mapping').length}</span>
                  <span class="metric-label">Orphan Tracker Mappings</span>
                </div>
                <div class="metric-box">
                  <span class="metric-val">{integrityReport.issues.filter(i => i.category === 'mangabaka_conflict').length}</span>
                  <span class="metric-label">MangaBaka Conflicts</span>
                </div>
                <div class="metric-box">
                  <span class="metric-val">{integrityReport.issues.filter(i => i.category === 'komga_ownership_anomaly').length}</span>
                  <span class="metric-label">Komga Ownership Anomalies</span>
                </div>
                <div class="metric-box">
                  <span class="metric-val">{integrityReport.issues.filter(i => i.category === 'duplicate_volume_record').length}</span>
                  <span class="metric-label">Duplicate Volume Records</span>
                </div>
                <div class="metric-box">
                  <span class="metric-val">{integrityReport.issues.filter(i => i.category === 'stuck_outbox_notification').length}</span>
                  <span class="metric-label">Stuck Outbox Items</span>
                </div>
                <div class="metric-box">
                  <span class="metric-val">{integrityReport.issues.filter(i => i.category === 'impossible_auto_acquire_state').length}</span>
                  <span class="metric-label">Auto-Acquire State Issues</span>
                </div>
              </div>

              {#if integrityReport.issues.length > 0}
                <!-- Filter and Search Toolbar -->
                <div class="integrity-filters-toolbar">
                  <div class="filter-group-row">
                    <div class="severity-pills" role="radiogroup" aria-label="Filter by Severity">
                      <button
                        type="button"
                        class="btn mini pill"
                        class:spot={integritySeverityFilter === 'ALL'}
                        onclick={() => { integritySeverityFilter = 'ALL'; }}
                      >
                        All ({integrityReport.issues.length})
                      </button>
                      <button
                        type="button"
                        class="btn mini pill sev-crit"
                        class:spot={integritySeverityFilter === 'CRITICAL'}
                        onclick={() => { integritySeverityFilter = 'CRITICAL'; }}
                      >
                        Critical ({integrityReport.issuesBySeverity['CRITICAL'] || 0})
                      </button>
                      <button
                        type="button"
                        class="btn mini pill sev-act"
                        class:spot={integritySeverityFilter === 'ACTION_REQUIRED'}
                        onclick={() => { integritySeverityFilter = 'ACTION_REQUIRED'; }}
                      >
                        Action Required ({integrityReport.issuesBySeverity['ACTION_REQUIRED'] || 0})
                      </button>
                      <button
                        type="button"
                        class="btn mini pill sev-warn"
                        class:spot={integritySeverityFilter === 'WARNING'}
                        onclick={() => { integritySeverityFilter = 'WARNING'; }}
                      >
                        Warning ({integrityReport.issuesBySeverity['WARNING'] || 0})
                      </button>
                      <button
                        type="button"
                        class="btn mini pill sev-info"
                        class:spot={integritySeverityFilter === 'INFO'}
                        onclick={() => { integritySeverityFilter = 'INFO'; }}
                      >
                        Info ({integrityReport.issuesBySeverity['INFO'] || 0})
                      </button>
                    </div>

                    <div class="category-pills" role="radiogroup" aria-label="Filter by Category">
                      <button
                        type="button"
                        class="btn mini pill"
                        class:spot={integrityCategoryFilter === 'ALL'}
                        onclick={() => { integrityCategoryFilter = 'ALL'; }}
                      >
                        All Categories
                      </button>
                      <button
                        type="button"
                        class="btn mini pill"
                        class:spot={integrityCategoryFilter === 'storage'}
                        onclick={() => { integrityCategoryFilter = 'storage'; }}
                      >
                        Storage
                      </button>
                      <button
                        type="button"
                        class="btn mini pill"
                        class:spot={integrityCategoryFilter === 'ownership'}
                        onclick={() => { integrityCategoryFilter = 'ownership'; }}
                      >
                        Ownership
                      </button>
                      <button
                        type="button"
                        class="btn mini pill"
                        class:spot={integrityCategoryFilter === 'identity'}
                        onclick={() => { integrityCategoryFilter = 'identity'; }}
                      >
                        Identity
                      </button>
                      <button
                        type="button"
                        class="btn mini pill"
                        class:spot={integrityCategoryFilter === 'notifications'}
                        onclick={() => { integrityCategoryFilter = 'notifications'; }}
                      >
                        Notifications
                      </button>
                      <button
                        type="button"
                        class="btn mini pill"
                        class:spot={integrityCategoryFilter === 'policy'}
                        onclick={() => { integrityCategoryFilter = 'policy'; }}
                      >
                        Policy
                      </button>
                    </div>
                  </div>

                  <div class="search-input-row">
                    <input
                      type="text"
                      class="integrity-search-input"
                      placeholder="Filter by series title, code, or message…"
                      bind:value={integritySearchQuery}
                    />
                    {#if integritySearchQuery}
                      <button
                        type="button"
                        class="btn mini outline"
                        onclick={() => { integritySearchQuery = ''; }}
                      >
                        Clear
                      </button>
                    {/if}
                  </div>
                </div>

                {@const filteredIssues = integrityReport.issues.filter(issue => {
                  if (integritySeverityFilter !== 'ALL' && issue.severity !== integritySeverityFilter) return false;
                  if (integrityCategoryFilter !== 'ALL' && issue.category.toLowerCase() !== integrityCategoryFilter.toLowerCase()) return false;
                  if (integritySearchQuery.trim()) {
                    const q = integritySearchQuery.trim().toLowerCase();
                    const titleMatch = issue.seriesTitle?.toLowerCase().includes(q) ?? false;
                    const msgMatch = issue.message.toLowerCase().includes(q);
                    const codeMatch = issue.code.toLowerCase().includes(q);
                    const detailsMatch = issue.details?.toLowerCase().includes(q) ?? false;
                    if (!titleMatch && !msgMatch && !codeMatch && !detailsMatch) return false;
                  }
                  return true;
                })}

                <div class="integrity-issues-list">
                  <h4>Filtered Issues ({filteredIssues.length} of {integrityReport.issues.length})</h4>
                  {#if filteredIssues.length === 0}
                    <div class="empty-state font-small muted">
                      No issues match the selected severity, category, or search filters.
                    </div>
                  {:else}
                    {#each filteredIssues as issue}
                      <div class="issue-item issue-sev-{issue.severity.toLowerCase()}">
                        <div class="issue-header">
                          <span class="badge badge-sev badge-sev-{issue.severity.toLowerCase()}">{issue.severity}</span>
                          <span class="issue-cat">{issue.category.replace(/_/g, ' ')}</span>
                          {#if issue.storageType === 'canonical_and_staging'}
                            <span class="badge badge-storage-dual" title="Present in canonical library and historical download staging">
                              Dual Storage: Canonical + Staging
                            </span>
                          {:else if issue.storageType === 'duplicate_canonical'}
                            <span class="badge badge-storage-dup" title="Multiple files in canonical storage">
                              Duplicate Canonical
                            </span>
                          {:else if issue.storageType === 'duplicate_staging'}
                            <span class="badge badge-storage-staging" title="Multiple files in download staging">
                              Duplicate Staging
                            </span>
                          {/if}
                          {#if issue.mangaId}
                            <a href="#/title/{issue.mangaId}" class="issue-title-link">
                              {issue.seriesTitle ? `${issue.seriesTitle} (ID #${issue.mangaId})` : `Manga #${issue.mangaId}`}
                              <Icon name="chevron-right" size={14} />
                            </a>
                          {:else if issue.seriesTitle}
                            <span class="issue-title">{issue.seriesTitle}</span>
                          {/if}
                        </div>
                        <div class="issue-desc">{issue.message}</div>
                        {#if issue.details}
                          <div class="issue-desc font-small muted monospace">{issue.details}</div>
                        {/if}
                        {#if issue.resolutionHint}
                          <div class="issue-hint">
                            <Icon name="info" size={13} />
                            <span><strong>Resolution:</strong> {issue.resolutionHint}</span>
                          </div>
                        {/if}
                      </div>
                    {/each}
                  {/if}
                </div>
              {/if}
            {/if}
          </section>
        {/if}

        {#if pipelineFilter === 'all' || pipelineFilter === 'diagnostics'}
          <!-- Staging Storage Hygiene & Cleanup Manifest (M7.1) -->
          <section class="card">
            <div class="card-header-row">
              <div>
                <h3>Staging Storage Hygiene & Cleanup Preview</h3>
                <span class="muted font-small">Safety-first inventory and preview manifest for staged/downloaded manga archives. Real deletion is strictly preview-only in M7.</span>
              </div>
              <div class="left-actions">
                <button class="btn small" onclick={loadStagingInventory} disabled={stagingLoading || stagingManifestLoading}>
                  <Icon name={stagingLoading ? 'spinner' : 'search'} size={16} />
                  {stagingLoading ? 'Scanning…' : 'Scan Staging'}
                </button>
                <button class="btn small spot" onclick={loadCleanupPreview} disabled={stagingLoading || stagingManifestLoading}>
                  <Icon name={stagingManifestLoading ? 'spinner' : 'check'} size={16} />
                  {stagingManifestLoading ? 'Generating…' : 'Cleanup Preview'}
                </button>
              </div>
            </div>

            {#if stagingError}
              <Notice error={stagingError} />
            {/if}

            {#if !stagingReport && !stagingLoading}
              <div class="empty-state">
                Click "Scan Staging" to inspect staged artifacts, active transfers, and verified duplicates.
              </div>
            {:else if stagingReport}
              <div class="analysis-summary-grid">
                <div class="metric-card">
                  <span class="metric-val">{stagingReport.totalFiles}</span>
                  <span class="metric-lbl">Total Files</span>
                </div>
                <div class="metric-card">
                  <span class="metric-val">{sized(stagingReport.totalStagingBytes)}</span>
                  <span class="metric-lbl">Staged Storage</span>
                </div>
                <div class="metric-card metric-already">
                  <span class="metric-val">{sized(stagingReport.activeTransferBytes)}</span>
                  <span class="metric-lbl">Active Transfers</span>
                </div>
                <div class="metric-card metric-matched">
                  <span class="metric-val">{sized(stagingReport.verifiedDuplicateBytes)}</span>
                  <span class="metric-lbl">Verified Duplicates</span>
                </div>
                <div class="metric-card metric-update">
                  <span class="metric-val">{sized(stagingReport.estimatedReclaimableBytes)}</span>
                  <span class="metric-lbl">Est. Reclaimable</span>
                </div>
              </div>

              {#if stagingManifest}
                <div class="integrity-summary-banner banner-warning">
                  <div class="banner-main">
                    <strong>⚠️ {stagingManifest.manifestId} — Execution Locked</strong>
                    {#if stagingManifest.verificationIdentity}
                      <span class="muted font-small">Verification ID: <code>{stagingManifest.verificationIdentity}</code></span>
                    {/if}
                    <span>{stagingManifest.notice}</span>
                  </div>
                </div>

                <div class="staging-manifest-summary">
                  <span class="pill-badge active">Eligible: {stagingManifest.eligibleItems.length}</span>
                  <span class="pill-badge paused">Blocked: {stagingManifest.blockedItems.length}</span>
                  <span class="muted font-small">Reclaimable if approved: {sized(stagingManifest.estimatedReclaimableBytes)}</span>
                </div>
              {/if}

              {#if stagingReport.items.length > 0}
                <div class="normalization-table-wrap">
                  {#each stagingReport.items as item}
                    <div class="diagnostic-card">
                      <div class="diag-header-row">
                        <div class="diag-title-area">
                          <strong>{item.fileName}</strong>
                          {#if item.seriesTitle}
                            <span class="muted font-small">({item.seriesTitle})</span>
                          {/if}
                        </div>
                        <div class="diag-badges">
                          {#if item.eligibilityDecision === 'ELIGIBLE'}
                            <span class="pill-badge active">Eligible</span>
                          {:else if item.eligibilityDecision === 'BLOCKED'}
                            <span class="pill-badge paused">Blocked</span>
                          {/if}
                          <span class="pill-badge {item.classification === 'PUBLISHED_VERIFIED_DUPLICATE' && !item.activeDependencyDetected ? 'active' : 'paused'}">
                            {item.classification}
                          </span>
                          {#if item.activeDependencyDetected}
                            <span class="pill-badge error">Active Chapter Ref</span>
                          {/if}
                          <span class="muted font-small">{sized(item.sizeBytes)}</span>
                        </div>
                      </div>
                      <div class="diag-body font-small">
                        <span>{item.reason}</span>
                        {#if item.sha256}
                          <span class="muted font-small">SHA-256: <code>{item.sha256.slice(0, 16)}…</code></span>
                        {/if}
                        {#if item.canonicalDestination}
                          <span class="muted">Canonical: <code>{item.canonicalDestination}</code></span>
                        {/if}
                        {#if item.canonicalSha256}
                          <span class="muted font-small">Canonical SHA: <code>{item.canonicalSha256.slice(0, 16)}…</code></span>
                        {/if}
                        {#if item.missingEvidence && item.missingEvidence.length > 0}
                          <div class="missing-evidence font-small text-danger">
                            <span>Missing evidence:</span>
                            <ul>
                              {#each item.missingEvidence as evidence}
                                <li>{evidence}</li>
                              {/each}
                            </ul>
                          </div>
                        {/if}
                      </div>
                    </div>
                  {/each}
                </div>
              {:else}
                <p class="muted font-small">No staged files found in downloads folder.</p>
              {/if}
            {/if}
          </section>

          <!-- IMAGE OPTIMIZER STATUS (M8.6) -->
          <section class="card">
            <div class="card-header-row">
              <div class="header-with-badge">
                <h3>Image Optimizer — RustyLibraryShrinker</h3>
                {#if storageOptimizerStatus}
                  <span class="pill-badge" class:active={storageOptimizerStatus.isAvailable} class:paused={!storageOptimizerStatus.isAvailable}>
                    {storageOptimizerStatus.isAvailable ? 'Available' : 'Unavailable'}
                  </span>
                {/if}
              </div>
              <button class="btn small" onclick={loadOptimizerStatus} disabled={optimizerStatusLoading}>
                <Icon name="refresh" size={14} /> Refresh Status
              </button>
            </div>
            {#if storageOptimizerStatus}
              <div class="optimizer-details-grid">
                <div class="opt-row"><span class="opt-k">Status:</span><span class="opt-v">{storageOptimizerStatus.message}</span></div>
                {#if storageOptimizerStatus.resolvedPath}
                  <div class="opt-row"><span class="opt-k">Binary Path:</span><code class="opt-v">{storageOptimizerStatus.resolvedPath}</code></div>
                {/if}
                {#if storageOptimizerStatus.version}
                  <div class="opt-row"><span class="opt-k">Version:</span><span class="opt-v">{storageOptimizerStatus.version}</span></div>
                {/if}
                <div class="opt-row"><span class="opt-k">Target Max Height:</span><span class="opt-v">{storageOptimizerStatus.defaultTargetHeight}px</span></div>
                <div class="opt-row"><span class="opt-k">WebP/JPEG Quality:</span><span class="opt-v">{storageOptimizerStatus.defaultImageQuality}%</span></div>
                <div class="opt-row"><span class="opt-k">Min Savings Required:</span><span class="opt-v">{storageOptimizerStatus.defaultMinSavingsPercent}%</span></div>
              </div>
            {:else}
              <p class="muted">Click "Refresh Status" to probe RustyLibraryShrinker installation and capabilities.</p>
            {/if}
          </section>

          <!-- LIBRARY DUPLICATES & OVERLAPS (M8.6) -->
          <section class="card">
            <div class="card-header-row">
              <h3>Library Duplicate Detection & Overlaps</h3>
              <button class="btn small" onclick={loadDuplicates} disabled={duplicatesLoading}>
                {#if duplicatesLoading}
                  <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span> Scanning…
                {:else}
                  <Icon name="search" size={14} /> Scan Duplicates
                {/if}
              </button>
            </div>

            <div class="integrity-summary-banner banner-info">
              <div class="banner-main">
                <strong>Zero Auto-Deletion Invariant:</strong>
                <span> All duplicate resolution recommendations are strictly non-destructive previews and require manual review. Canonical media files are never automatically pruned.</span>
              </div>
            </div>

            {#if duplicatesError}
              <Notice error={duplicatesError} />
            {/if}

            {#if duplicatesReport}
              <div class="analysis-summary-grid">
                <div class="metric-card">
                  <span class="metric-val">{duplicatesReport.totalDuplicateGroups}</span>
                  <span class="metric-lbl">Total Issues</span>
                </div>
                <div class="metric-card metric-matched">
                  <span class="metric-val">{duplicatesReport.exactDuplicatesCount}</span>
                  <span class="metric-lbl">Exact Hash Duplicates</span>
                </div>
                <div class="metric-card metric-update">
                  <span class="metric-val">{duplicatesReport.semanticDuplicatesCount}</span>
                  <span class="metric-lbl">Semantic Duplicates</span>
                </div>
                <div class="metric-card metric-already">
                  <span class="metric-val">{duplicatesReport.chapterVolumeOverlapsCount}</span>
                  <span class="metric-lbl">Chapter Overlaps</span>
                </div>
                <div class="metric-card">
                  <span class="metric-val">{sized(duplicatesReport.potentialReclaimableBytes)}</span>
                  <span class="metric-lbl">Reclaimable Space</span>
                </div>
              </div>

              {#if duplicatesReport.groups.length === 0}
                <div class="empty-state">
                  No duplicate archives or chapter overlaps detected in your library.
                </div>
              {:else}
                <div class="duplicates-groups-list">
                  {#each duplicatesReport.groups as group}
                    <div class="diagnostic-card">
                      <div class="diag-header-row">
                        <div class="diag-title-area">
                          <span class="pill-badge">{group.groupType.replace(/_/g, ' ')}</span>
                          <strong>{group.seriesTitle}</strong>
                          {#if group.volumeNumber != null}
                            <span class="vol-tag">Vol {group.volumeNumber}</span>
                          {/if}
                          {#if group.potentialReclaimableBytes > 0}
                            <span class="muted font-small">({sized(group.potentialReclaimableBytes)} reclaimable)</span>
                          {/if}
                        </div>
                      </div>

                      <div class="candidate-preview-bar">
                        <Icon name="check" size={14} />
                        <span>{group.resolutionRecommendation}</span>
                      </div>

                      {#if group.items && group.items.length > 0}
                        <div class="items-preview-list">
                          {#each group.items as item}
                            <div class="item-preview-row">
                              <div class="item-name-block">
                                {#if item.itemId === group.recommendedKeepId}
                                  <span class="pill-badge active">KEEP (RECOMMENDED)</span>
                                {:else}
                                  <span class="pill-badge paused">DUPLICATE</span>
                                {/if}
                                <span class="font-small"><code>{item.fileName}</code></span>
                              </div>
                              <div class="item-meta-block font-small muted">
                                <span>{item.format}</span>
                                {#if item.isDigital}<span>Digital</span>{/if}
                                <span>{sized(item.sizeBytes)}</span>
                              </div>
                            </div>
                          {/each}
                        </div>
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}
            {/if}
          </section>

          <section class="card">
            <h3>Recent Pipeline Activity</h3>
            {#if pipelineEvents.length === 0}
              <p class="muted">No recent pipeline executions recorded. When downloads complete, their processing and publication will appear here.</p>
            {:else}
              <div class="events-list">
                {#each pipelineEvents as evt (evt.id)}
                  <div class="event-item">
                    <div class="event-header">
                      <span class="event-series">{evt.series}</span>
                      <span class="event-release">{evt.release_name}</span>
                      <span class="event-badge badge-{evt.outcome}">
                        {evt.outcome === 'published' ? 'Published' : evt.outcome === 'already_published' ? 'Already in Library' : evt.outcome === 'conflict' ? 'Conflict (Preserved)' : evt.outcome === 'error' ? 'Error' : evt.outcome}
                      </span>
                      <span class="event-time">{ago(evt.timestamp * 1000)}</span>
                    </div>
                    <div class="event-details">
                      <span class="detail">MangaBaka: <em>{evt.mangabaka_status}</em></span>
                      <span class="detail">Optimizer: <em>{evt.optimizer_status}</em></span>
                      <span class="detail">Komga: <em>{evt.komga_status}</em></span>
                    </div>
                    {#if evt.destination}
                      <div class="event-dest">Canonical: <code>{evt.destination}</code></div>
                    {/if}
                    {#if evt.error}
                      <div class="event-error">{evt.error}</div>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          </section>
        {/if}
      {:else if tab.id === 'notifications'}
        <section class="card">
          <div class="card-header-row">
            <h3>ntfy Notification Service</h3>
            <div class="card-actions">
              <button class="btn small" onclick={handleTestNotification} disabled={ntfyTesting || !ntfyTopic}>
                <Icon name="bell" size={16} />
                {ntfyTesting ? 'Testing…' : 'Send Test Notification'}
              </button>
              <button class="btn small primary" onclick={handleSaveNotifications} disabled={ntfySaving}>
                <Icon name="check" size={16} />
                {ntfySaving ? 'Saving…' : 'Save'}
              </button>
            </div>
          </div>

          {#if ntfyTestResult}
            <div class="set stack">
              <div class="test-result-box {ntfyTestResult.success ? 'test-success' : 'test-failure'}">
                <strong>{ntfyTestResult.success ? '✓ Notification Sent' : '✗ Test Failed'}</strong>
                <span>{ntfyTestResult.message}</span>
              </div>
            </div>
          {/if}

          <div class="set">
            <div class="what">
              <strong>Enable ntfy Notifications</strong>
              <span>Deliver real-time push alerts to mobile devices and browsers via ntfy for releases, completions, and attention events.</span>
            </div>
            <div class="how">
              {#if ntfyConfig}
                {@render toggle(ntfyConfig.enabled, 'Enable notifications', () => {
                  if (ntfyConfig) ntfyConfig.enabled = !ntfyConfig.enabled;
                })}
              {/if}
            </div>
          </div>

          <div class="set stack">
            <div class="what">
              <strong>ntfy Server URL</strong>
              <span>Base URL of the ntfy instance (default: <code>https://ntfy.sh</code>).</span>
            </div>
            <div class="how wide">
              <label class="field grow">
                <input type="text" bind:value={ntfyServerUrl} placeholder="https://ntfy.sh" autocomplete="off" />
              </label>
            </div>
          </div>

          <div class="set stack">
            <div class="what">
              <strong>ntfy Topic</strong>
              <span>Topic name where notifications will be sent (e.g. <code>my-manga-library</code>).</span>
            </div>
            <div class="how wide">
              <label class="field grow">
                <input type="text" bind:value={ntfyTopic} placeholder="topic-name" autocomplete="off" />
              </label>
            </div>
          </div>

          <div class="set stack">
            <div class="what">
              <strong>Access Token (Optional)</strong>
              <span>Authentication token for protected or self-hosted topics. Keep empty if public.</span>
            </div>
            <div class="how wide">
              <label class="field grow">
                <input type="password" bind:value={ntfyToken} placeholder={ntfyConfig?.hasToken ? '•••••••• (configured)' : 'Optional token'} autocomplete="off" />
              </label>
            </div>
          </div>
        </section>

        <section class="card">
          <h3>Notification Preferences</h3>
          <p class="section-desc">Select which logical events trigger push notifications.</p>

          <div class="set">
            <div class="what">
              <strong>📚 New Volume Detected</strong>
              <span>When a new official volume is released on MangaBaka for followed series.</span>
            </div>
            <div class="how">
              {#if ntfyConfig}
                {@render toggle(ntfyConfig.prefNewVolume, 'New Volume', () => {
                  if (ntfyConfig) ntfyConfig.prefNewVolume = !ntfyConfig.prefNewVolume;
                })}
              {/if}
            </div>
          </div>

          <div class="set">
            <div class="what">
              <strong>🔎 Release Available</strong>
              <span>When high-confidence download candidates are discovered for missing volumes.</span>
            </div>
            <div class="how">
              {#if ntfyConfig}
                {@render toggle(ntfyConfig.prefReleaseAvailable, 'Release Available', () => {
                  if (ntfyConfig) ntfyConfig.prefReleaseAvailable = !ntfyConfig.prefReleaseAvailable;
                })}
              {/if}
            </div>
          </div>

          <div class="set">
            <div class="what">
              <strong>✅ Acquisition Complete</strong>
              <span>When a volume has been downloaded, canonicalized, published, and verified in Komga.</span>
            </div>
            <div class="how">
              {#if ntfyConfig}
                {@render toggle(ntfyConfig.prefAcquisitionComplete, 'Acquisition Complete', () => {
                  if (ntfyConfig) ntfyConfig.prefAcquisitionComplete = !ntfyConfig.prefAcquisitionComplete;
                })}
              {/if}
            </div>
          </div>

          <div class="set">
            <div class="what">
              <strong>⚠️ Action Required</strong>
              <span>When series matching is ambiguous, metadata fails, or review is required.</span>
            </div>
            <div class="how">
              {#if ntfyConfig}
                {@render toggle(ntfyConfig.prefActionRequired, 'Action Required', () => {
                  if (ntfyConfig) ntfyConfig.prefActionRequired = !ntfyConfig.prefActionRequired;
                })}
              {/if}
            </div>
          </div>
        </section>

        <section class="card">
          <div class="card-header-row">
            <div>
              <h3>Notification Event History & Deduplication</h3>
              <p class="muted font-small">Recorded high-level system notification events. Active events can be acknowledged or marked resolved.</p>
            </div>
            <div class="card-actions">
              <div class="pill-group" role="radiogroup" aria-label="Notification events filter">
                <button
                  class="pill"
                  class:active={notificationEventsActiveOnly}
                  onclick={() => {
                    notificationEventsActiveOnly = true;
                    loadNotificationEvents();
                  }}
                >
                  Active Only
                </button>
                <button
                  class="pill"
                  class:active={!notificationEventsActiveOnly}
                  onclick={() => {
                    notificationEventsActiveOnly = false;
                    loadNotificationEvents();
                  }}
                >
                  All Events
                </button>
              </div>
              <button class="btn small" onclick={loadNotificationEvents} disabled={notificationEventsLoading}>
                <Icon name="refresh" size={16} />
                {notificationEventsLoading ? 'Loading…' : 'Refresh'}
              </button>
            </div>
          </div>

          {#if notificationEvents.length === 0}
            <div class="empty-state">
              {notificationEventsActiveOnly ? 'No active notification events requiring attention.' : 'No notification events recorded.'}
            </div>
          {:else}
            <div class="events-audit-list">
              {#each notificationEvents as evt (evt.id)}
                {@const isResolved = !evt.isActive}
                {@const statusStr = evt.isActive ? (evt.outboxStatus ?? 'Active') : 'Resolved'}
                <div class="event-audit-card" class:resolved={isResolved} class:action-required={evt.eventType === 'ACTION_REQUIRED'}>
                  <div class="event-audit-head">
                    <div class="event-audit-badges">
                      <span class="badge badge-sev badge-sev-{evt.severity.toLowerCase()}">{evt.severity}</span>
                      <span class="badge badge-type">{evt.eventType}</span>
                      <span class="badge badge-stat badge-stat-{statusStr.toLowerCase()}">{statusStr}</span>
                      <span class="monospace font-small event-key">#{evt.id}: {evt.seriesId}</span>
                    </div>
                    <div class="event-audit-time">
                      <span>Created {ago(evt.createdAt * 1000)}</span>
                    </div>
                  </div>

                  <div class="event-audit-body">
                    <div class="event-audit-subj">
                      <strong>{evt.seriesTitle}</strong>
                      {#if evt.volumeNumber !== null && evt.volumeNumber !== undefined}
                        <span>Vol. {evt.volumeNumber}</span>
                      {/if}
                      {#if evt.edition && evt.edition !== 'standard'}
                        <span class="muted font-small">({evt.edition})</span>
                      {/if}
                      {#if evt.mangabakaId}
                        <span class="muted font-small">[MB #{evt.mangabakaId}]</span>
                      {/if}
                    </div>
                    <div class="event-audit-msg">{evt.message}</div>
                    {#if evt.outboxLastError}
                      <div class="suppression-note">
                        <Icon name="info" size={13} />
                        <span>Outbox Error: {evt.outboxLastError}</span>
                      </div>
                    {/if}
                  </div>

                  <div class="event-audit-foot">
                    <div class="event-audit-timestamps font-small muted">
                      {#if evt.outboxDeliveredAt}
                        <span>Delivered {ago(evt.outboxDeliveredAt * 1000)}</span>
                      {/if}
                      {#if evt.resolvedAt}
                        <span>Resolved {ago(evt.resolvedAt * 1000)}</span>
                      {/if}
                    </div>
                    {#if evt.isActive}
                      <button
                        class="btn mini spot"
                        disabled={resolvingEventId === evt.id}
                        onclick={() => handleResolveNotificationEvent(evt.id)}
                        title="Mark event as resolved / dismiss notice"
                      >
                        {#if resolvingEventId === evt.id}
                          <span class="spin"><Icon name="spinner" size={13} stroke={2.5} /></span> Resolving…
                        {:else}
                          <Icon name="check" size={13} /> Resolve
                        {/if}
                      </button>
                    {/if}
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </section>

        <section class="card">
          <div class="card-header-row">
            <h3>Notification Outbox History</h3>
            <button class="btn small" onclick={loadOutbox} disabled={ntfyOutboxLoading}>
              <Icon name="link" size={16} />
              {ntfyOutboxLoading ? 'Refreshing…' : 'Refresh'}
            </button>
          </div>

          {#if ntfyOutbox.length === 0}
            <div class="empty-state">No recent notification outbox entries.</div>
          {:else}
            <div class="outbox-list">
              {#each ntfyOutbox as item}
                <div class="outbox-row">
                  <div class="outbox-header">
                    <span class="status-badge status-{item.eventType.toLowerCase()}">{item.eventType}</span>
                    <span class="status-badge status-{item.status}">{item.status}</span>
                    <span class="hist-time">{ago(item.createdAt)}</span>
                  </div>
                  {#if item.lastError}
                    <div class="event-error">{item.lastError}</div>
                  {/if}
                  <div class="outbox-details">
                    <span>Topic: <code>{item.destination}</code></span>
                    <span>Attempts: {item.attempts}</span>
                    {#if item.deliveredAt}
                      <span>Delivered: {ago(item.deliveredAt)}</span>
                    {/if}
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </section>
      {:else if tab.id === 'sources'}
        <section class="card">
          <h3>Sources</h3>
          {#if localDir}
            <div class="set stack">
              <div class="what">
                <strong>Local folder</strong>
                <span>One folder per title; inside it a .cbz, a .zip or a folder of images per chapter. They show up under Browse, in “Local”.</span>
              </div>
              <code>{localDir}</code>
            </div>
          {/if}
          <div class="set">
            <div class="what"><strong>Extensions and stores</strong><span>Install, update and remove the extensions that fetch titles.</span></div>
            <div class="how"><a class="btn" href="#/extensions"><Icon name="puzzle" size={18} />Open extensions</a></div>
          </div>
        </section>

        {#if session.admin}
          <section class="card">
            <h3>Cloudflare</h3>
            <form class="set" onsubmit={saveSolver}>
              <div class="what">
                <strong>Solver</strong>
                <span>
                  Some sources sit behind a Cloudflare check that only a browser passes. Mango hands the check to a FlareSolverr, or anything
                  that answers like one, at this address. Empty: no solver.
                </span>
              </div>
              <label class="field grow">
                <span class="sr">The solver's address</span>
                <input type="url" bind:value={solver.url} placeholder="http://localhost:8191" autocomplete="off" />
              </label>
              <label class="field seconds">
                <span class="sr">Seconds a check may take</span>
                <input type="number" bind:value={solver.timeoutSecs} min="5" max="300" title="Seconds a check may take" />
              </label>
              <button class="btn ink" disabled={savingSolver}>
                {#if savingSolver}
                  <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                {/if}
                Save
              </button>
            </form>
            {#if solverError || solverSaved}
              <div class="set stack">
                <Notice error={solverError} />
                {#if solverSaved}<p class="muted" role="status">Saved. The sources use it from their next call.</p>{/if}
              </div>
            {/if}
          </section>
        {/if}

        <section class="card">
          <h3>Reading apps</h3>
          <div class="set stack">
            <div class="what">
              <strong>OPDS catalog</strong>
              <span>
                An app that reads OPDS catalogs can browse this library and fetch its chapters. Give it this
                address{#if session.login}, with your name and password{/if}.
              </span>
            </div>
            <code>{home}opds</code>
          </div>
        </section>

        <section class="card">
          <h3>Backup</h3>
          <div class="set">
            <div class="what"><strong>From Mihon or Suwayomi</strong><span>Bring a library, its categories and reading progress in from a backup file.</span></div>
            <div class="how"><a class="btn" href="#/import"><Icon name="download" size={18} />Import a backup</a></div>
          </div>
          <div class="set">
            <div class="what">
              <strong>Export the library</strong>
              <span>A .tachibk file with the library, its categories, reading progress, history and tracker links. Mihon and Suwayomi read it too.</span>
            </div>
            <div class="how"><a class="btn" href="api/export" download><Icon name="upload" size={18} />Export a backup</a></div>
          </div>
        </section>
      {:else if tab.id === 'trackers'}
        {#each trackers as t (t.id)}
          <section class="card">
            <h3>{t.name}</h3>
            {#if t.signedIn}
              <div class="set">
                <div class="what">
                  <strong>Signed in{t.userName ? ` as ${t.userName}` : ''}</strong>
                  <span>Link a title from its own page. Reading a chapter then moves it along on {t.name}.</span>
                </div>
                <div class="how"><button class="btn" disabled={working === t.id} onclick={() => signOutTracker(t)}>Sign out</button></div>
              </div>
            {:else if t.login === 'password'}
              <form class="set" onsubmit={(e) => signIn(e, t)}>
                <label class="field grow">
                  <span class="sr">Your name on {t.name}</span>
                  <input bind:value={typed[t.id].name} placeholder={t.name === 'Kitsu' ? 'E-mail' : 'User name'} autocomplete="off" required />
                </label>
                <label class="field grow">
                  <span class="sr">Your password on {t.name}</span>
                  <input type="password" bind:value={typed[t.id].secret} placeholder="Password" autocomplete="off" required />
                </label>
                <button class="btn ink" disabled={working === t.id}>Sign in</button>
              </form>
              <div class="set stack"><p class="muted">The password goes to {t.name} once and is not kept. Mango keeps the token it gets back.</p></div>
            {:else if t.login === 'token'}
              <div class="set stack">
                <div class="what">
                  <strong>1. Make an API client</strong>
                  <span>On {t.name}, under Settings, Developer, make a client with the redirect URL below, and put the id it gets here.</span>
                </div>
                <code>{ANILIST_PIN}</code>
                <div class="how">
                  <label class="field">
                    <span class="sr">The client's id</span>
                    <input value={clientOf(t)} inputmode="numeric" placeholder="Client id" autocomplete="off" onchange={(e) => setClient(t, e.currentTarget.value)} />
                  </label>
                  {#if clientOf(t)}
                    <a class="btn" href="https://anilist.co/api/v2/oauth/authorize?client_id={encodeURIComponent(clientOf(t))}&response_type=token" target="_blank" rel="noreferrer">
                      <Icon name="globe" size={18} />Get a token
                    </a>
                  {/if}
                </div>
              </div>
              <form class="set" onsubmit={(e) => signIn(e, t)}>
                <label class="field grow">
                  <span class="sr">The token {t.name} shows</span>
                  <input bind:value={typed[t.id].secret} placeholder="2. Paste the token {t.name} shows" autocomplete="off" required />
                </label>
                <button class="btn ink" disabled={working === t.id}>Sign in</button>
              </form>
            {:else}
              <div class="set stack">
                <div class="what">
                  <strong>1. Register Mango</strong>
                  <span>On {t.name}, under Account Settings, API, make a client of the type “other” with the redirect URL below, and put its client id here.</span>
                </div>
                <code>{home}</code>
                <div class="how">
                  <label class="field">
                    <span class="sr">The client's id</span>
                    <input value={clientOf(t)} placeholder="Client id" autocomplete="off" onchange={(e) => setClient(t, e.currentTarget.value)} />
                  </label>
                  <button class="btn ink" disabled={!clientOf(t) || working === t.id} onclick={() => leaveFor(t)}><Icon name="globe" size={18} />2. Sign in at {t.name}</button>
                </div>
              </div>
              <form class="set" onsubmit={(e) => finish(e, t)}>
                <div class="what"><strong>Did not come back here?</strong><span>Paste the address {t.name} sent the browser to.</span></div>
                <label class="field grow">
                  <span class="sr">The address {t.name} sent the browser to</span>
                  <input bind:value={typed[t.id].secret} placeholder="https://…?code=…" autocomplete="off" required />
                </label>
                <button class="btn" disabled={working === t.id}>Finish</button>
              </form>
            {/if}
          </section>
        {/each}
        <section class="card">
          <div class="card-header-row">
            <h3>Komga Reading Progress Synchronization</h3>
            <div class="sync-actions">
              <button class="btn" onclick={triggerDryRun} disabled={syncRunning || syncLoading}>
                <Icon name="eye" size={16} /> {syncLoading ? 'Running…' : 'Run Dry Run'}
              </button>
              {#if session.admin}
                <button class="btn spot" onclick={triggerLiveSync} disabled={syncRunning || syncLoading}>
                  <Icon name="upload" size={16} /> Sync Now
                </button>
              {/if}
            </div>
          </div>

          <p class="muted">
            Synchronizes authoritative reading progress from Komga to AniList and MangaBaka.
            Komga is the single source of truth for completed volumes and chapters.
            Progress only moves forward; dry runs perform zero tracker writes.
          </p>

          <div class="path-status-row" style="margin-bottom: 12px;">
            <span class="status-badge {pipeline.komga_enabled && pipeline.has_komga_api_key ? 'status-valid' : !pipeline.komga_enabled ? 'status-not_configured' : 'status-missing'}">
              {#if !pipeline.komga_enabled}
                Komga: Disabled
              {:else if pipeline.has_komga_api_key}
                Komga: Connected
              {:else}
                Komga: No API Key
              {/if}
            </span>
            <span class="status-badge {trackers.find(x => x.name === 'AniList')?.signedIn ? 'status-valid' : 'status-not_configured'}">
              AniList: {trackers.find(x => x.name === 'AniList')?.signedIn ? 'Signed In' : 'Not Signed In'}
            </span>
            {#if !pipeline.has_mangabaka_token}
              <span class="status-badge status-not_configured">
                MangaBaka: No Token
              </span>
            {:else if mangabakaTestResult ? mangabakaTestResult.ok : pipeline.mangabaka_validated === true}
              <span class="status-badge status-valid">
                MangaBaka: Validated
              </span>
            {:else if mangabakaTestResult ? !mangabakaTestResult.ok : pipeline.mangabaka_validated === false}
              <span class="status-badge status-missing">
                MangaBaka: Validation Failed
              </span>
            {:else}
              <span class="status-badge status-not_configured">
                MangaBaka: Token Configured (Unverified)
              </span>
            {/if}
            {#if syncRunning}
              <span class="status-badge status-valid">Sync in progress…</span>
            {/if}
          </div>

          {#if session.admin}
            <div class="set" style="border-top: 1px solid var(--ink); padding-top: 12px; margin-top: 12px;">
              <div class="what">
                <strong>Scheduled background synchronization</strong>
                <span>Periodically synchronizes progress from Komga to trackers. Disabled by default.</span>
              </div>
              <div class="how">
                {@render toggle(syncSchedule.enabled, 'Enable scheduled sync', toggleSyncSchedule)}
              </div>
            </div>
            {#if syncSchedule.enabled}
              <div class="set">
                <div class="what">
                  <strong>Sync interval</strong>
                  <span>How often scheduled synchronization evaluates reading progress.</span>
                </div>
                <div class="how">
                  <label class="field">
                    <input type="number" min="1" max="168" bind:value={syncSchedule.intervalHours} onchange={saveSchedule} style="width: 80px;" />
                    <span>hours</span>
                  </label>
                </div>
              </div>
            {/if}
          {/if}

          {#if syncError}
            <Notice error={syncError} />
          {/if}

          {#if syncSummary}
            <div class="sync-summary-box">
              <div class="sync-summary-header">
                <strong>{syncSummary.isDryRun ? 'Dry Run Evaluation Result' : 'Sync Run Completed'}</strong>
                <span class="badge {syncSummary.errors > 0 ? 'badge-error' : 'badge-ok'}">
                  {syncSummary.totalSeries} series processed
                </span>
              </div>
              <div class="sync-summary-counts">
                <span><strong>{syncSummary.isDryRun ? syncSummary.dryRunUpdates : syncSummary.updated}</strong> {syncSummary.isDryRun ? 'would update' : 'updated'}</span>
                <span><strong>{syncSummary.noChange}</strong> unchanged</span>
                {#if syncSummary.trackerAhead}
                  <span><strong>{syncSummary.trackerAhead}</strong> tracker ahead</span>
                {/if}
                <span><strong>{syncSummary.skipped}</strong> skipped</span>
                {#if syncSummary.untouchedSeries}
                  <span class="muted">({syncSummary.untouchedSeries} untouched)</span>
                {/if}
                <span><strong>{syncSummary.errors}</strong> errors</span>
              </div>
            </div>
          {/if}

          {#if syncDiagnostics.length > 0}
            <div class="sync-history-container" style="margin-bottom: 16px;">
              <div class="sync-history-header">
                <h4>Unresolved &amp; Ambiguous Titles ({syncDiagnostics.length})</h4>
                <button class="btn" onclick={loadSyncDiagnostics} style="font-size: 11px; padding: 2px 8px;">Refresh</button>
              </div>
              <div class="sync-records-list">
                {#each syncDiagnostics as diag (diag.komgaSeriesId + diag.tracker)}
                  <div class="sync-record-item">
                    <div class="rec-top">
                      <span class="rec-title">{diag.komgaTitle}</span>
                      <span class="rec-tracker badge">{diag.tracker.toUpperCase()}</span>
                      <span class="status-badge status-failed">{diag.confidence}</span>
                      <button
                        class="btn"
                        style="font-size: 11px; padding: 2px 8px; margin-left: auto;"
                        onclick={() => {
                          manualMappingModal = {
                            open: true,
                            komgaSeriesId: diag.komgaSeriesId,
                            komgaTitle: diag.komgaTitle,
                            tracker: diag.tracker,
                            trackerId: '',
                            editionId: '',
                          };
                        }}
                      >
                        Set Manual ID
                      </button>
                      <button
                        class="btn"
                        style="font-size: 11px; padding: 2px 8px;"
                        onclick={() => handleResetMapping(diag.komgaSeriesId, diag.tracker)}
                      >
                        Reset to Auto
                      </button>
                    </div>
                    <div class="rec-body">
                      <span class="rec-msg">{diag.reason}</span>
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/if}

          {#if syncHistory.length > 0}
            <div class="sync-history-container">
              <div class="sync-history-header">
                <h4>Recent Sync Audit Records</h4>
                <div style="display: flex; gap: 8px; align-items: center;">
                  {#if latestRunRecordsCount > 0}
                    <div class="audit-filter-pills" style="display: inline-flex; border: 1px solid var(--ink); border-radius: 4px; overflow: hidden; font-size: 11px;">
                      <button
                        type="button"
                        class="btn-pill"
                        onclick={() => (auditFilter = 'latest')}
                        style="padding: 2px 8px; background: {auditFilter === 'latest' ? 'var(--ink)' : 'transparent'}; color: {auditFilter === 'latest' ? 'var(--paper)' : 'inherit'}; border: none; cursor: pointer;"
                      >
                        Latest Run ({latestRunRecordsCount})
                      </button>
                      <button
                        type="button"
                        class="btn-pill"
                        onclick={() => (auditFilter = 'all')}
                        style="padding: 2px 8px; background: {auditFilter === 'all' ? 'var(--ink)' : 'transparent'}; color: {auditFilter === 'all' ? 'var(--paper)' : 'inherit'}; border: none; cursor: pointer;"
                      >
                        All ({syncHistory.length})
                      </button>
                    </div>
                  {/if}
                  <button class="btn" onclick={loadSyncHistory} style="font-size: 11px; padding: 2px 8px;">Refresh</button>
                </div>
              </div>
              <div class="sync-records-list">
                {#each filteredSyncHistory as rec (rec.id)}
                  <div class="sync-record-item">
                    <div class="rec-top">
                      <span class="rec-title">{rec.komgaSeriesTitle}</span>
                      <span class="rec-tracker badge">{rec.tracker.toUpperCase()}</span>
                      {#if rec.dimension}
                        <span class="badge" style="font-size: 10px; opacity: 0.8;">{rec.dimension.toUpperCase()}</span>
                      {/if}
                      {#if isLatestRun(rec)}
                        <span class="status-badge status-valid" style="font-size: 10px; padding: 1px 6px;">
                          Latest Run
                        </span>
                      {/if}
                      <span class="status-badge {rec.category === 'updated' || rec.category === 'would_update' || rec.category === 'dry_run_would_update' ? 'status-valid' : rec.category.startsWith('skipped') ? 'status-not_configured' : rec.category.includes('error') ? 'status-failed' : 'status-not_configured'}">
                        {rec.category.replace(/_/g, ' ')}
                      </span>
                      <span class="rec-time">{ago(rec.timestamp)}</span>
                    </div>
                    <div class="rec-body">
                      {#if rec.oldProgress != null || rec.newProgress != null}
                        <span class="rec-progress">
                          Progress: {rec.oldProgress ?? 0} &rarr; {rec.newProgress ?? rec.oldProgress}
                          {#if rec.statusChange}
                            <em>({rec.statusChange})</em>
                          {/if}
                        </span>
                      {/if}
                      <span class="rec-msg">{rec.message}</span>
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/if}

          {#if manualMappingModal.open}
            <div class="modal-backdrop" style="position: fixed; top: 0; left: 0; right: 0; bottom: 0; background: rgba(0,0,0,0.6); z-index: 1000; display: flex; align-items: center; justify-content: center;">
              <div class="card" style="max-width: 480px; width: 90%; padding: 24px; box-shadow: 0 4px 20px rgba(0,0,0,0.3);">
                <h3>Manual Tracker Mapping Override</h3>
                <p class="muted" style="margin-bottom: 16px; font-size: 13px;">Explicit manual overrides take permanent precedence over automated title matching.</p>
                <div style="margin-bottom: 12px; font-size: 14px;">
                  <strong>Title:</strong> {manualMappingModal.komgaTitle} ({manualMappingModal.tracker.toUpperCase()})
                </div>
                <div style="margin-bottom: 12px;">
                  <label class="field grow" style="display: block; margin-bottom: 4px; font-size: 13px;">
                    <span>Tracker ID (AniList media ID or MangaBaka ID):</span>
                    <input type="text" class="input" style="width: 100%; padding: 8px; margin-top: 4px;" bind:value={manualMappingModal.trackerId} placeholder="e.g. 106130" required />
                  </label>
                </div>
                {#if manualMappingModal.tracker === 'mangabaka'}
                  <div style="margin-bottom: 16px;">
                    <label class="field grow" style="display: block; margin-bottom: 4px; font-size: 13px;">
                      <span>Edition Collection ID (Optional):</span>
                      <input type="text" class="input" style="width: 100%; padding: 8px; margin-top: 4px;" bind:value={manualMappingModal.editionId} placeholder="e.g. digital-standard" />
                    </label>
                  </div>
                {/if}
                <div style="display: flex; gap: 8px; justify-content: flex-end; margin-top: 20px;">
                  <button class="btn" onclick={() => (manualMappingModal.open = false)}>Cancel</button>
                  <button class="btn ink" onclick={handleSetManualMapping}>Save Manual Override</button>
                </div>
              </div>
            </div>
          {/if}
        </section>
      {:else if tab.id === 'account'}
        {#if session.account}
          <section class="card">
            <h3>Plan and billing</h3>
            <div class="set">
              <div class="what">
                <strong>Your account</strong>
                <span>Your plan, the room your downloads take, billing and your email are kept on your account page.</span>
              </div>
              <div class="how"><a class="btn spot" href={session.account}>Manage your account</a></div>
            </div>
          </section>
        {/if}
        {#if session.login}
          <section class="card">
            <h3>Password</h3>
            <form class="set" onsubmit={changePassword}>
              <input class="sr" type="text" name="username" autocomplete="username" value="mango" tabindex="-1" aria-hidden="true" readonly />
              <label class="field grow">
                <span class="sr">The password in use now</span>
                <input type="password" bind:value={current} placeholder="Current password" autocomplete="current-password" required />
              </label>
              <label class="field grow">
                <span class="sr">The new password</span>
                <input type="password" bind:value={renewed} placeholder="New password, 8 characters or more" autocomplete="new-password" minlength="8" required />
              </label>
              <button class="btn ink">Change password</button>
            </form>
            {#if accountError || changed}
              <div class="set stack">
                <Notice error={accountError} />
                {#if changed}<p class="muted" role="status">The password is changed. Every other browser was signed out.</p>{/if}
              </div>
            {/if}
          </section>

          <section class="card">
            <h3>This browser</h3>
            <div class="set">
              <div class="what">
                <strong>Sign out</strong>
                <span>Signed in as {session.name}. Mango asks for the password again here.</span>
              </div>
              <div class="how"><button class="btn" onclick={() => signOut().catch((e) => (accountError = message(e)))}>Sign out</button></div>
            </div>
          </section>

          {#if session.admin}
            <section class="card">
              <h3>Accounts</h3>
              <div class="set stack">
                <div class="what">
                  <span>
                    Each account has its own stores and extensions, library, reading progress, downloads and settings. An admin is one who
                    may change this list.
                  </span>
                </div>
              </div>
              {#each users as user (user.id)}
                <div class="set">
                  <div class="what">
                    <strong>{user.name}</strong>
                    <span>{user.admin ? 'Admin' : 'Reader'}{user.name === session.name ? ' · you' : ''} · added {ago(user.createdAt)}</span>
                  </div>
                  {#if rekeyed === user.id}
                    <form class="how" onsubmit={(event) => setKey(event, user.id)}>
                      <label class="field grow">
                        <span class="sr">A new password for {user.name}</span>
                        <input type="password" bind:value={rekey} placeholder="New password" autocomplete="new-password" minlength="8" required />
                      </label>
                      <button class="btn ink">Set</button>
                      <button class="btn" type="button" onclick={() => (rekeyed = null)}>Cancel</button>
                    </form>
                  {:else if user.name !== session.name}
                    <div class="how">
                      <button class="btn" onclick={() => ((rekeyed = user.id), (rekey = ''))}>New password</button>
                      {#if user.id !== 1}
                        <button class="btn" disabled={userBusy} onclick={() => toggleAdmin(user)}>
                          {user.admin ? 'Make reader' : 'Make admin'}
                        </button>
                        <button class="btn" disabled={userBusy} onclick={() => removeUser(user.id)}>
                          Remove
                        </button>
                      {/if}
                    </div>
                  {/if}
                </div>
              {/each}
              <form class="set" onsubmit={addUser}>
                <label class="field grow">
                  <span class="sr">The new account's name</span>
                  <input type="text" bind:value={newcomer.name} placeholder="Name" autocomplete="off" maxlength="32" required />
                </label>
                <label class="field grow">
                  <span class="sr">The new account's password</span>
                  <input
                    type="password"
                    bind:value={newcomer.password}
                    placeholder="Password, 8 characters or more"
                    autocomplete="new-password"
                    minlength="8"
                    required
                  />
                </label>
                <label class="check"><input type="checkbox" bind:checked={newcomer.admin} />Admin</label>
                <button class="btn ink" disabled={userBusy}>
                  {#if userBusy}
                    <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                  {/if}
                  Add account
                </button>
              </form>
              {#if usersError}
                <div class="set stack"><Notice error={usersError} /></div>
              {/if}
            </section>

            {#if session.signup}
              <section class="card">
                <h3>Invitations</h3>
                <div class="set stack">
                  <div class="what">
                    <span>
                      Whoever holds an invitation makes their own account on the main page, as a reader. Each one opens once and holds a
                      week.
                    </span>
                  </div>
                </div>
                {#if made?.code}
                  <div class="set stack">
                    <div class="what">
                      <strong>Send this link. It is shown this once.</strong>
                      <span>Or the code alone, to type in: {made.code}</span>
                    </div>
                    <div class="how wide">
                      <label class="field grow">
                        <span class="sr">The invitation's link</span>
                        <input type="text" value={inviteLink} readonly onfocus={(event) => event.currentTarget.select()} />
                      </label>
                      <button class="btn ink" onclick={copyInvite}>{linkCopied ? 'Copied' : 'Copy link'}</button>
                    </div>
                  </div>
                {/if}
                {#each invites as invite (invite.id)}
                  <div class="set">
                    <div class="what">
                      <strong>{invite.note || 'No note'}</strong>
                      <span>{inviteState(invite)}</span>
                    </div>
                    <div class="how">
                      <button class="btn" disabled={inviteBusy} onclick={() => removeInvite(invite.id)}>{invite.usedAt ? 'Forget' : 'Take back'}</button>
                    </div>
                  </div>
                {/each}
                <form class="set" onsubmit={addInvite}>
                  <label class="field grow">
                    <span class="sr">Who the invitation is for</span>
                    <input type="text" bind:value={invitee} placeholder="Who it is for: only admins see this" autocomplete="off" maxlength="80" />
                  </label>
                  <button class="btn ink" disabled={inviteBusy}>
                    {#if inviteBusy}
                      <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                    {/if}
                    Make an invitation
                  </button>
                </form>
                {#if invitesError}
                  <div class="set stack"><Notice error={invitesError} /></div>
                {/if}
              </section>
            {/if}
          {/if}
        {:else}
          <p class="warn"><strong>Login is switched off.</strong> Anyone who can reach this address can use Mango. Take <code>MANGO_LOGIN=off</code> away to ask for a password.</p>
        {/if}
      {:else if v}
        <section class="card">
          <h3>Tour</h3>
          <div class="set">
            <div class="what">
              <strong>Show the tour again</strong>
              <span>The cards a new account sees first, and where the reader's tap zones are the next time a chapter opens.</span>
            </div>
            <div class="how"><button class="btn" onclick={againTour}>Show the tour</button></div>
          </div>
        </section>

        <section class="card">
          <h3>This version</h3>
          <div class="set">
            <div class="what">
              <strong>Mango {v.current}</strong>
              <span>{[v.commit, v.platform, v.docker ? 'Docker' : ''].filter(Boolean).join(' · ')}</span>
            </div>
            <div class="how">
              <button class="btn" disabled={checking || !v.canCheck} onclick={check}><Icon name="refresh" size={18} />{checking ? 'Checking' : 'Check now'}</button>
            </div>
          </div>
          <div class="set stack">
            {#if v.updateAvailable && v.latest}
              <div class="warn" role="status">
                <p><strong>Mango {v.latest.version} is out.</strong></p>
                {#if v.docker}
                  <p>Pull the new image and start the container again: <code>docker compose pull && docker compose up -d</code></p>
                {:else if v.latest.download}
                  <p>Stop Mango, unpack the download over the old files, and start it again. The data folder stays as it is.</p>
                  <p><a class="btn spot" href={v.latest.download.url} rel="noreferrer"><Icon name="download" size={18} />{v.latest.download.name}</a></p>
                {:else}
                  <p>There is no ready-made download for {v.platform}. <a href={v.latest.url} target="_blank" rel="noreferrer">Open the release</a>.</p>
                {/if}
              </div>
            {:else if v.checkedAt && !v.error}
              <p class="muted" role="status">This is the newest release. Checked {ago(v.checkedAt)}.</p>
            {/if}
            {#if v.error}<p class="muted" role="status">{v.error}</p>{/if}
            {#if !v.canCheck}
              <p class="muted">This build was made by hand and does not know where releases are listed. Set <code>MANGO_RELEASES_REPO</code> to <code>owner/name</code> to look for new ones.</p>
            {/if}
            <Notice error={versionError} />
            <p class="muted">{v.checkHours ? `Looked for every ${v.checkHours} hours.` : 'Looked for only when asked.'}</p>
          </div>
        </section>

        {#if v.releases.length}
          <section class="card">
            <h3>Releases and their notes</h3>
            <ul class="rows releases">
              {#each v.releases as release (release.version)}
                <li>
                  <span class="main">
                    <strong>{release.name}</strong>
                    <span>{[day(release.publishedAt), release.prerelease ? 'pre-release' : '', release.version === v.current ? 'this one' : ''].filter(Boolean).join(' · ')}</span>
                    {#if release.notes.trim()}<pre>{release.notes.trim()}</pre>{/if}
                  </span>
                  <a class="btn" href={release.url} target="_blank" rel="noreferrer">Open</a>
                </li>
              {/each}
            </ul>
          </section>
        {/if}
      {:else}
        <p class="muted">{version ? `Mango ${version}` : ''}</p>
      {/if}
    </div>
  </div>
</div>

{#if editingProfile}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={() => (editingProfile = null)}>
    <div class="modal-content profile-editor-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-header">
        <h3>{isNewProfile ? 'New Acquisition Profile' : `Edit Profile: ${editingProfile.name}`}</h3>
        <button class="btn icon close-btn" onclick={() => (editingProfile = null)} aria-label="Close">
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body stack-fields">
        <label class="field">
          <span class="label">Profile Name</span>
          <input type="text" class="input" bind:value={editingProfile.name} placeholder="e.g. High Quality Manga" />
        </label>

        <div class="field-row-split">
          <label class="field">
            <span class="label">Language</span>
            <select class="input" bind:value={editingProfile.language}>
              <option value="en">English (en)</option>
              <option value="zh">Chinese (zh)</option>
              <option value="any">Any Language</option>
            </select>
          </label>
          <label class="field">
            <span class="label">Release Preference</span>
            <select class="input" bind:value={editingProfile.preferredReleaseType}>
              <option value="volume">Complete Volumes</option>
              <option value="chapter">Chapters</option>
              <option value="any">Any Compatible</option>
            </select>
          </label>
        </div>

        <div class="field-row-split">
          <label class="field">
            <span class="label">Collection Policy</span>
            <select class="input" bind:value={editingProfile.collectionPolicy}>
              <option value="smart">Smart (Batch when >= 3 missing)</option>
              <option value="prefer_collections">Prefer Collections</option>
              <option value="prefer_individual">Prefer Individual Volumes</option>
              <option value="never_collections">Never Collections</option>
            </select>
          </label>
          <label class="field">
            <span class="label">Minimum Seeders</span>
            <input type="number" class="input" min="0" max="100" bind:value={editingProfile.minSeeders} />
          </label>
        </div>

        <div class="field-row-split">
          <label class="field">
            <span class="label">Max Duplicate Volumes Allowed</span>
            <input type="number" class="input" min="0" max="50" bind:value={editingProfile.maxUnnecessaryDuplicateCoverage} />
          </label>
          <label class="field">
            <span class="label">Zero Seeder Policy</span>
            <select class="input" bind:value={editingProfile.allowZeroSeeders}>
              <option value="no">Disallow (Reject 0 seeders)</option>
              <option value="ask">Review Recommended</option>
              <option value="yes">Allow</option>
            </select>
          </label>
        </div>

        <div class="checkbox-row-group">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editingProfile.preferDigital} />
            <span>Prefer Digital releases over physical scans</span>
          </label>
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editingProfile.trustedOnly} />
            <span>Trusted uploaders only</span>
          </label>
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editingProfile.allowMixedCollections} />
            <span>Allow mixed volume + chapter collections</span>
          </label>
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editingProfile.allowUnsupportedEditions} />
            <span>Allow variant editions (Full Color, Deluxe, etc.)</span>
          </label>
        </div>

        <div class="disabled-feature-notice">
          <Icon name="info" size={16} />
          <span>Auto-acquire remains strictly OFF in this milestone. Autonomous acquisition is disabled.</span>
        </div>
      </div>

      <div class="modal-footer">
        <button class="btn" onclick={() => (editingProfile = null)}>Cancel</button>
        <button class="btn spot" onclick={saveEditingProfile}>
          <Icon name="check" size={16} />
          {isNewProfile ? 'Create Profile' : 'Save Changes'}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .head-left {
    display: flex;
    align-items: baseline;
    gap: 20px;
    flex-wrap: wrap;
    margin-bottom: 8px;
  }

  .category-switcher {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
  }

  .pipeline-section-bar {
    position: sticky;
    top: 0;
    z-index: 10;
    background: var(--paper);
    padding: 8px 0;
    margin-bottom: 16px;
    border-bottom: 2px solid var(--ink);
  }

  .layout {
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    align-items: start;
    gap: 36px;
  }

  /* ---------- the groups ---------- */

  .rail {
    position: sticky;
    top: 16px;
    display: flex;
    flex-direction: column;
    border: 3px solid var(--ink);
    background: var(--surface);
    box-shadow: var(--sh-5);
  }

  .rail a {
    display: flex;
    flex: none;
    align-items: center;
    gap: 10px;
    min-height: 48px;
    padding: 0 14px;
    border-bottom: 2px solid var(--ink);
    font-size: 13px;
    font-weight: 900;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    white-space: nowrap;
  }

  .rail a:last-child {
    border-bottom: 0;
  }

  .rail a[aria-current='page'] {
    background: var(--ink);
    color: var(--paper);
  }

  .rail a:focus-visible {
    outline-offset: -5px;
  }

  .rail .tag {
    margin-left: auto;
  }

  .pane {
    display: flex;
    flex-direction: column;
    gap: 24px;
    min-width: 0;
  }

  /* ---------- a card of settings, one per row ---------- */

  .card {
    border: 3px solid var(--ink);
    background: var(--surface);
    box-shadow: var(--sh-5);
  }

  h3 {
    padding: 9px 16px;
    background: var(--ink);
    color: var(--paper);
    font-size: 12px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .set {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 10px 24px;
    padding: 14px 16px;
    border-bottom: 2px solid var(--ink);
  }

  .set:last-child {
    border-bottom: 0;
  }

  .set.stack {
    flex-direction: column;
    align-items: flex-start;
  }

  .what {
    display: flex;
    flex: 1 1 240px;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .stack .what {
    flex: none;
  }

  .what strong {
    font-weight: 900;
  }

  .what span {
    color: var(--muted);
    font-size: 13px;
  }

  .how {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  /* In a stacked row, as wide as the row: for a field that holds a whole address. */
  .how.wide {
    align-self: stretch;
  }

  /* ---------- controls ---------- */

  .switch {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    min-height: 44px;
    padding: 0;
    border: 0;
    background: none;
    font-size: 12px;
    font-weight: 900;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .track {
    position: relative;
    width: 52px;
    height: 28px;
    border: 2px solid var(--ink);
    background: var(--paper);
  }

  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 20px;
    height: 20px;
    background: var(--ink);
    transition: transform 0.12s;
  }

  .switch[aria-checked='true'] .track {
    background: var(--spot);
  }

  .switch[aria-checked='true'] .knob {
    background: var(--on-spot);
    transform: translateX(24px);
  }

  .state {
    min-width: 2.4em;
    text-align: left;
  }

  @media (prefers-reduced-motion: reduce) {
    .knob {
      transition: none;
    }
  }

  .seg {
    display: inline-flex;
    border: 2px solid var(--ink);
  }

  .seg button {
    min-height: 40px;
    padding: 0 16px;
    border: 0;
    border-right: 2px solid var(--ink);
    background: var(--surface);
    font-size: 14px;
    font-weight: 700;
  }

  .seg button:last-child {
    border-right: 0;
  }

  .seg button[aria-pressed='true'] {
    background: var(--ink);
    color: var(--paper);
  }

  .seg button:focus-visible {
    outline-offset: -5px;
  }

  .swatch[aria-pressed='true'] {
    box-shadow: var(--sh-4);
  }

  .picker {
    gap: 10px;
  }

  input[type='color'] {
    width: 36px;
    height: 28px;
    padding: 0;
    border: 2px solid var(--ink);
    background: none;
  }

  .sample {
    display: inline-flex;
    align-items: center;
    min-height: 44px;
    padding: 0 14px;
    border: 2px solid;
    font-weight: 700;
  }

  .warn {
    max-width: 640px;
    padding: 12px 14px;
    border: 3px solid var(--ink);
    background: var(--paper);
  }

  .warn p + p {
    margin-top: 8px;
  }

  .swap {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: min(560px, 100%);
    font-size: 12px;
    font-weight: 900;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  textarea {
    width: 100%;
    padding: 10px;
    border: 2px solid var(--ink);
    background: var(--paper);
    color: var(--ink);
    font: 13px/1.4 ui-monospace, Consolas, monospace;
    letter-spacing: 0;
    text-transform: none;
  }

  code {
    overflow-wrap: anywhere;
  }

  /* ---------- lists inside a card ---------- */

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    flex-wrap: wrap;
    padding: 6px 16px;
  }

  /* Keeps a name whole: the buttons beside it drop to the next line first. */
  li .main {
    flex-basis: 140px;
  }

  .releases li {
    align-items: flex-start;
  }

  .releases li:last-child {
    border-bottom: 0;
  }

  .rename {
    flex-direction: row;
    flex-wrap: wrap;
    gap: 8px;
  }

  pre {
    margin: 8px 0 0;
    font: 13px/1.5 ui-monospace, Consolas, monospace;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .rename .field,
  .grow {
    flex: 1 1 220px;
    box-shadow: none;
  }

  .seconds {
    flex: 0 0 96px;
    box-shadow: none;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 700;
  }

  /* On a narrow screen the groups turn into one strip that scrolls sideways. */
  @media (max-width: 760px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
      gap: 20px;
    }

    .rail {
      top: var(--safe-top);
      z-index: 2;
      flex-direction: row;
      overflow-x: auto;
      box-shadow: none;
      scrollbar-width: none;
    }

    /* A row of choices takes the whole line, each choice an equal share of it. */
    .how:has(> .seg) {
      flex: 1 1 100%;
    }

    /* The lines between them are the ink showing through, so they hold when the row has to wrap. */
    .seg {
      flex: 1;
      flex-wrap: wrap;
      gap: 2px;
      background: var(--ink);
    }

    .seg button {
      flex: 1 0 auto;
      padding: 0 6px;
      border-right: 0;
    }

    .rail a {
      border-right: 2px solid var(--ink);
      border-bottom: 0;
    }

    .rail a:last-child {
      border-right: 0;
    }
  }

  .action-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    margin: 16px 0;
  }

  .events-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-top: 12px;
  }

  .event-item {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px 14px;
    border: 2px solid var(--ink);
    background: var(--surface);
    box-shadow: var(--sh-2);
  }

  .event-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
  }

  .event-series {
    font-weight: 900;
    font-size: 14px;
  }

  .event-release {
    font-size: 13px;
    color: var(--muted);
  }

  .event-time {
    margin-left: auto;
    font-size: 12px;
    color: var(--muted);
  }

  .event-badge {
    display: inline-flex;
    align-items: center;
    padding: 2px 8px;
    border: 2px solid var(--ink);
    font-size: 11px;
    font-weight: 800;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .badge-published {
    background: var(--spot);
    color: var(--on-spot);
  }

  .badge-already_published {
    background: var(--paper);
    color: var(--ink);
  }

  .badge-conflict {
    background: #ffb703;
    color: #000;
  }

  .badge-error {
    background: #e63946;
    color: #fff;
  }

  .badge-skipped {
    background: var(--surface);
    color: var(--muted);
  }

  .event-details {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    font-size: 12px;
  }

  .event-details .detail em {
    font-style: normal;
    font-weight: 700;
  }

  .event-dest {
    font-size: 12px;
    word-break: break-all;
  }

  .event-dest code {
    font-size: 11px;
    background: var(--paper);
    padding: 2px 4px;
    border: 1px solid var(--ink);
  }

  .event-error {
    color: #e63946;
    font-size: 12px;
    font-weight: 700;
  }

  .card-header-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 8px;
  }

  .card-header-row h3 {
    margin: 0;
  }

  .path-status-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
    margin-top: 4px;
    font-size: 12px;
  }

  .status-badge {
    display: inline-flex;
    align-items: center;
    padding: 2px 8px;
    border: 1px solid var(--ink);
    font-size: 11px;
    font-weight: 800;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .status-valid {
    background: var(--spot);
    color: var(--on-spot);
  }

  .status-missing,
  .status-not_a_directory,
  .status-not_writable,
  .status-not_a_file,
  .status-not_executable,
  .status-failed {
    background: #e63946;
    color: #fff;
  }

  .status-not_configured {
    background: var(--surface);
    color: var(--muted);
  }

  .status-msg {
    color: var(--muted);
    font-size: 12px;
  }

  .text-spot {
    color: var(--spot);
    font-weight: 700;
  }

  .sync-actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .sync-summary-box {
    margin-top: 12px;
    padding: 12px;
    background: var(--surface);
    border: 2px solid var(--ink);
  }

  .sync-summary-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
  }

  .sync-summary-counts {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
    font-size: 13px;
  }

  .sync-history-container {
    margin-top: 16px;
    border-top: 1px solid var(--ink);
    padding-top: 12px;
  }

  .sync-history-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
  }

  .sync-history-header h4 {
    margin: 0;
  }

  .sync-records-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: 400px;
    overflow-y: auto;
  }

  .sync-record-item {
    background: var(--surface);
    border: 1px solid var(--ink);
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }

  .rec-top {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .rec-title {
    font-weight: 700;
  }

  .rec-tracker {
    font-size: 10px;
    padding: 1px 6px;
    background: var(--paper);
    border: 1px solid var(--ink);
  }

  .rec-time {
    margin-left: auto;
    color: var(--muted);
    font-size: 11px;
  }

  .rec-body {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    color: var(--muted);
  }

  .rec-progress {
    font-weight: 600;
    color: var(--ink);
  }

  .rec-msg {
    font-size: 11px;
  }

  /* Acquisition Profiles & Sources */
  .card-header-flex {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: 1rem;
  }

  .profiles-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 1rem;
    margin-top: 1rem;
  }

  .profile-card {
    background: var(--card-bg, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .profile-card.default-card {
    border-color: var(--spot, #ff7a00);
    box-shadow: 0 0 8px rgba(255, 122, 0, 0.15);
  }

  .profile-card-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    padding-bottom: 0.5rem;
  }

  .profile-actions {
    display: flex;
    gap: 0.35rem;
  }

  .profile-attrs-table {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    font-size: 0.85rem;
  }

  .attr-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .attr-k {
    color: var(--muted, #888);
  }

  .attr-v {
    font-weight: 500;
  }

  .sources-health-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    margin-top: 1rem;
  }

  .source-health-card {
    background: var(--card-bg, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 6px;
    padding: 0.75rem 1rem;
  }

  .source-health-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.5rem;
  }

  .source-health-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 1.25rem;
    font-size: 0.82rem;
    color: var(--muted, #888);
  }

  /* Modal */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 1rem;
  }

  .profile-editor-modal {
    background: var(--bg, #18181b);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.15));
    border-radius: 10px;
    width: 100%;
    max-width: 560px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 16px 36px rgba(0, 0, 0, 0.4);
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
  }

  .modal-body {
    padding: 1.25rem;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 0.75rem;
    padding: 1rem 1.25rem;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    background: rgba(0, 0, 0, 0.2);
  }

  .field-row-split {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1rem;
  }

  .checkbox-row-group {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }

  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.88rem;
    cursor: pointer;
  }

  .disabled-feature-notice {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.65rem 0.85rem;
    background: rgba(255, 122, 0, 0.08);
    border: 1px solid rgba(255, 122, 0, 0.25);
    border-radius: 6px;
    font-size: 0.82rem;
    color: var(--spot, #ff7a00);
  }

  .staging-manifest-summary {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    margin: 0.75rem 0;
    flex-wrap: wrap;
  }

  /* Library Normalization & Maintenance Styles */
  .library-select-row {
    display: flex;
    gap: 0.75rem;
    align-items: center;
  }

  .analysis-summary-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
    gap: 0.65rem;
    margin: 1rem 0;
  }

  .analysis-summary-grid .metric-card {
    background: var(--shade);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.25rem;
    text-align: center;
  }

  .analysis-summary-grid .metric-val {
    font-size: 1.4rem;
    font-weight: 700;
  }

  .analysis-summary-grid .metric-lbl {
    font-size: 0.72rem;
    color: var(--muted, #888);
  }

  .metric-matched .metric-val { color: #22c55e; }
  .metric-already .metric-val { color: #3b82f6; }
  .metric-update .metric-val { color: var(--spot, #ff7a00); }
  .metric-ambiguous .metric-val { color: #eab308; }
  .metric-unmatched .metric-val { color: #a855f7; }
  .metric-skipped .metric-val { color: #888; }
  .metric-error .metric-val { color: #ef4444; }

  .maintenance-actions-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 1rem;
    padding: 0.75rem 0;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    margin-bottom: 1rem;
  }

  .left-actions {
    display: flex;
    gap: 0.5rem;
  }

  .normalization-table-wrap {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    max-height: 480px;
    overflow-y: auto;
    margin-bottom: 1.5rem;
  }

  .series-norm-card {
    background: var(--shade);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 0.75rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .series-norm-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .series-norm-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .series-stats {
    font-size: 0.78rem;
    color: var(--muted, #888);
  }

  .books-norm-list {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding-left: 0.5rem;
    border-left: 2px solid rgba(255, 255, 255, 0.08);
  }

  .book-norm-row {
    display: flex;
    flex-wrap: wrap;
    min-width: 0;
    justify-content: space-between;
    align-items: center;
    padding: 0.35rem 0.5rem;
    border-radius: 4px;
    font-size: 0.82rem;
  }

  .book-norm-row.needs-update {
    background: rgba(255, 122, 0, 0.06);
  }

  .book-row-left {
    display: flex;
    flex-wrap: wrap;
    min-width: 0;
    align-items: center;
    gap: 0.5rem;
  }

  .checkbox-spacer {
    width: 14px;
    display: inline-block;
  }

  .book-row-right {
    display: flex;
    flex-wrap: wrap;
    min-width: 0;
    align-items: center;
    gap: 0.5rem;
  }

  .diff-badge {
    font-size: 0.72rem;
    background: rgba(255, 122, 0, 0.15);
    color: var(--ink);
    padding: 1px 6px;
    border-radius: 4px;
    max-width: 100%;
    overflow-wrap: anywhere;
  }

  .diff-badge summary {
    cursor: pointer;
    font-weight: 700;
  }

  .history-section {
    margin-top: 1.5rem;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    padding-top: 1rem;
  }

  .history-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.75rem;
  }

  .history-header h4 {
    margin: 0;
  }

  .history-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-height: 280px;
    overflow-y: auto;
  }

  .history-item {
    background: var(--shade);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 0.65rem 0.85rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .hist-top {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .hist-time {
    margin-left: auto;
    font-size: 0.75rem;
    color: var(--muted, #888);
  }

  .hist-actions {
    display: flex;
    margin-top: 0.25rem;
  }

  .status-badge.status-needs_update {
    background: rgba(255, 122, 0, 0.15);
    color: var(--spot, #ff7a00);
  }

  .status-badge.status-already_normalized {
    background: rgba(59, 130, 246, 0.15);
    color: #3b82f6;
  }

  .status-badge.status-skipped_provider_not_applicable {
    background: rgba(255, 255, 255, 0.06);
    color: #888;
  }

  .status-badge.status-matched {
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
  }

  .status-badge.status-ambiguous {
    background: rgba(234, 179, 8, 0.15);
    color: #eab308;
  }

  .status-badge.status-unmatched {
    background: rgba(168, 85, 247, 0.15);
    color: #a855f7;
  }

  .status-badge.status-reverted {
    background: rgba(100, 116, 139, 0.15);
    color: #94a3b8;
  }

  .test-result-box {
    padding: 0.75rem 1rem;
    border-radius: 6px;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.85rem;
  }

  .test-result-box.test-success {
    background: rgba(34, 197, 94, 0.12);
    border: 1px solid rgba(34, 197, 94, 0.3);
    color: #22c55e;
  }

  .test-result-box.test-failure {
    background: rgba(239, 68, 68, 0.12);
    border: 1px solid rgba(239, 68, 68, 0.3);
    color: #ef4444;
  }

  .outbox-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }

  .outbox-row {
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 0.65rem 0.85rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .outbox-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .outbox-details {
    display: flex;
    flex-wrap: wrap;
    gap: 1rem;
    font-size: 0.75rem;
    color: var(--muted, #888);
  }

  .status-badge.status-delivered {
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
  }

  .status-badge.status-pending {
    background: rgba(59, 130, 246, 0.15);
    color: #3b82f6;
  }

  .status-badge.status-sending {
    background: rgba(234, 179, 8, 0.15);
    color: #eab308;
  }

  .status-badge.status-retry_wait {
    background: rgba(255, 122, 0, 0.15);
    color: var(--spot, #ff7a00);
  }

  .status-badge.status-permanently_failed {
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
  }

  .status-badge.status-suppressed {
    background: rgba(255, 255, 255, 0.06);
    color: #888;
  }

  .opentome-status-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 12px;
    background: var(--surface-2, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 12px 14px;
    width: 100%;
  }

  .ot-meta-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .ot-k {
    font-size: 0.75rem;
    color: var(--subtle, #888);
  }

  .ot-v {
    font-size: 0.85rem;
  }

  .ot-path {
    font-size: 0.75rem;
    word-break: break-all;
  }

  .opentome-btn-row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  /* Library Integrity Audit Styles */
  .integrity-summary-banner {
    border-radius: 6px;
    padding: 12px 16px;
    margin-bottom: 14px;
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
  }

  .integrity-summary-banner.banner-healthy {
    background: rgba(34, 197, 94, 0.1);
    border-color: rgba(34, 197, 94, 0.3);
    color: #22c55e;
  }

  .integrity-summary-banner.banner-warning {
    background: rgba(234, 179, 8, 0.1);
    border-color: rgba(234, 179, 8, 0.3);
    color: #eab308;
  }

  .banner-main {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 0.9rem;
  }

  .integrity-metrics-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 10px;
    margin-bottom: 16px;
  }

  .metric-box {
    background: var(--surface-2, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .metric-val {
    font-size: 1.4rem;
    font-weight: 800;
    line-height: 1;
  }

  .metric-label {
    font-size: 0.75rem;
    color: var(--muted, #888);
  }

  .integrity-issues-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .issue-item {
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 10px 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    background: var(--surface-1, rgba(255, 255, 255, 0.015));
  }

  .issue-item.issue-sev-critical {
    border-color: rgba(239, 68, 68, 0.35);
    background: rgba(239, 68, 68, 0.04);
  }

  .issue-item.issue-sev-warning {
    border-color: rgba(234, 179, 8, 0.35);
    background: rgba(234, 179, 8, 0.04);
  }

  .issue-header {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .issue-cat {
    font-size: 0.8rem;
    font-weight: 600;
    text-transform: capitalize;
  }

  .issue-title-link, .issue-title {
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--ink);
  }

  .issue-desc {
    font-size: 0.85rem;
    color: var(--dim, #ccc);
  }

  .issue-hint {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.78rem;
    color: var(--muted, #888);
    background: var(--surface-2, rgba(255, 255, 255, 0.03));
    padding: 4px 8px;
    border-radius: 4px;
  }

  /* M6.12 Integrity UX Filters & Storage Badges */
  .integrity-filters-toolbar {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 14px;
    padding: 10px 14px;
    background: var(--surface-2, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
  }

  .filter-group-row {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
    align-items: center;
  }

  .severity-pills, .category-pills {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    align-items: center;
  }

  .search-input-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .integrity-search-input {
    flex: 1;
    min-width: 220px;
    font-size: 0.85rem;
    padding: 6px 12px;
    background: var(--surface-1, rgba(255, 255, 255, 0.05));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.1));
    border-radius: 4px;
    color: var(--ink);
  }

  .badge-storage-dual {
    background: rgba(147, 51, 234, 0.15);
    border: 1px solid rgba(147, 51, 234, 0.4);
    color: #c084fc;
    font-size: 0.72rem;
    padding: 2px 6px;
    border-radius: 4px;
    font-weight: 600;
  }

  .badge-storage-dup {
    background: rgba(239, 68, 68, 0.15);
    border: 1px solid rgba(239, 68, 68, 0.4);
    color: #f87171;
    font-size: 0.72rem;
    padding: 2px 6px;
    border-radius: 4px;
    font-weight: 600;
  }

  .badge-storage-staging {
    background: rgba(234, 179, 8, 0.15);
    border: 1px solid rgba(234, 179, 8, 0.4);
    color: #facc15;
    font-size: 0.72rem;
    padding: 2px 6px;
    border-radius: 4px;
    font-weight: 600;
  }

  /* Notification Events Audit Styles */
  .events-audit-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 10px;
  }

  .event-audit-card {
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 10px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--surface-1, rgba(255, 255, 255, 0.02));
  }

  .event-audit-card.action-required {
    border-color: rgba(234, 179, 8, 0.35);
  }

  .event-audit-card.resolved {
    opacity: 0.65;
  }

  .event-audit-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .event-audit-badges {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .badge-sev {
    font-size: 10px;
    font-weight: 700;
    padding: 1px 6px;
    border-radius: 3px;
    text-transform: uppercase;
  }

  .badge-sev-critical {
    background: #ef4444;
    color: #fff;
  }

  .badge-sev-warning {
    background: #eab308;
    color: #000;
  }

  .badge-sev-info {
    background: #0284c7;
    color: #fff;
  }

  .badge-type {
    background: var(--shade, rgba(255, 255, 255, 0.1));
    font-size: 11px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 3px;
  }

  .badge-stat {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 3px;
    font-weight: 600;
  }

  .badge-stat-pending {
    background: rgba(59, 130, 246, 0.15);
    color: #3b82f6;
  }

  .badge-stat-delivered {
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
  }

  .badge-stat-resolved {
    background: rgba(255, 255, 255, 0.1);
    color: var(--dim, #aaa);
  }

  .event-key {
    color: var(--muted, #888);
  }

  .event-audit-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .event-audit-subj {
    font-size: 0.9rem;
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
  }

  .event-manga-link {
    color: var(--spot, #0284c7);
    font-weight: 600;
    text-decoration: none;
  }

  .event-manga-link:hover {
    text-decoration: underline;
  }

  .event-audit-msg {
    font-size: 0.82rem;
    color: var(--dim, #ccc);
  }

  .suppression-note {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.78rem;
    color: var(--muted, #888);
    background: var(--surface-2, rgba(255, 255, 255, 0.03));
    padding: 3px 6px;
    border-radius: 4px;
    margin-top: 2px;
  }

  .event-audit-foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.05));
    padding-top: 6px;
    margin-top: 2px;
  }

  .event-audit-timestamps {
    display: flex;
    gap: 12px;
    align-items: center;
  }

  .optimizer-details-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: 0.65rem 1.25rem;
    font-size: 0.85rem;
    background: rgba(0, 0, 0, 0.2);
    padding: 0.85rem 1rem;
    border-radius: 8px;
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  }

  .opt-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
  }

  .opt-k {
    color: var(--muted, #8b949e);
  }

  .opt-v {
    font-weight: 500;
  }

  .duplicates-groups-list {
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
    margin-top: 0.75rem;
  }

  .group-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-bottom: 0.4rem;
  }

  .recommendation-bar {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    background: rgba(34, 197, 94, 0.08);
    border: 1px solid rgba(34, 197, 94, 0.2);
    border-radius: 6px;
    padding: 0.4rem 0.65rem;
    font-size: 0.8rem;
    color: #86efac;
    margin-bottom: 0.5rem;
  }

  .items-preview-list {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .item-preview-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
    background: rgba(0, 0, 0, 0.15);
    padding: 0.35rem 0.65rem;
    border-radius: 4px;
  }

  .item-name-block {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .item-meta-block {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  /* ---------- Monitored Tracking Lists (M11) ---------- */
  .tracked-statuses-group {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }

  .tracked-status-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 32px;
    padding: 4px 14px;
    border-radius: 9999px;
    border: 2px solid var(--shade, rgba(0, 0, 0, 0.2));
    background: var(--surface, var(--paper));
    color: var(--dim, inherit);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
    user-select: none;
  }

  .tracked-status-btn:hover:not(:disabled) {
    border-color: var(--ink);
    color: var(--ink);
  }

  .tracked-status-btn.selected,
  .tracked-status-btn[aria-pressed='true'] {
    border-color: var(--ink);
    background: var(--ink);
    color: var(--paper);
    font-weight: 700;
  }

  .tracked-status-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .tracked-status-btn:focus-visible {
    outline: 2px solid var(--ink);
    outline-offset: 2px;
  }

  .tracked-summary {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    font-size: 13px;
    color: var(--dim, #666);
  }

  .tracked-summary strong {
    color: var(--ink);
  }

  .tracked-summary .saving-indicator {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--dim, #666);
    font-style: italic;
  }
</style>
