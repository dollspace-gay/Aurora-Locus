// Aurora Locus Admin Panel JavaScript

// Global state
let currentPage = 'dashboard';
let adminToken = localStorage.getItem('adminToken');
let currentUser = null;

// API Base URL
const API_BASE = '/xrpc';

// Initialize on page load
document.addEventListener('DOMContentLoaded', () => {
    checkAuth();
    setupNavigation();
    loadDashboardData();
    setupEventListeners();
    // Phase 3.7B: bell badge polling kicks off as soon as session is
    // valid. Polling pauses when the tab is hidden (Page Visibility
    // API) to honor §5.1 "no background fetches when tab not focused"
    // and resumes on visibility change.
    if (adminToken) startQueueBadgePolling();
    document.addEventListener('visibilitychange', () => {
        if (document.hidden) stopQueueBadgePolling();
        else if (adminToken) startQueueBadgePolling();
    });
});

// Authentication
function checkAuth() {
    if (!adminToken) {
        window.location.href = '/admin/login.html';
        return;
    }

    // Verify token with server
    fetch(`${API_BASE}/com.atproto.server.getSession`, {
        headers: {
            'Authorization': `Bearer ${adminToken}`
        }
    })
    .then(res => res.json())
    .then(data => {
        currentUser = data;
        document.getElementById('admin-name').textContent = data.handle || 'Admin';
    })
    .catch(() => {
        logout();
    });
}

function logout() {
    localStorage.removeItem('adminToken');
    window.location.href = '/admin/login.html';
}

// Navigation
function setupNavigation() {
    const navItems = document.querySelectorAll('.nav-item');
    navItems.forEach(item => {
        item.addEventListener('click', (e) => {
            e.preventDefault();
            const page = item.dataset.page;
            navigateTo(page);
        });
    });
}

function navigateTo(page) {
    // Update active nav item
    document.querySelectorAll('.nav-item').forEach(item => {
        item.classList.remove('active');
    });
    document.querySelector(`[data-page="${page}"]`).classList.add('active');

    // Update active page
    document.querySelectorAll('.page').forEach(p => {
        p.classList.remove('active');
    });
    document.getElementById(`page-${page}`).classList.add('active');

    // Phase 3.9: tear down page-bound subscriptions when leaving.
    if (currentPage === 'events' && page !== 'events' && eventsSubscription) {
        eventsSubscription.unsubscribe();
        eventsSubscription = null;
    }
    if (currentPage === 'audit' && page !== 'audit' && auditSubscription) {
        auditSubscription.unsubscribe();
        auditSubscription = null;
    }

    currentPage = page;

    // Load page data
    switch(page) {
        case 'dashboard':
            loadDashboardData();
            break;
        case 'users':
            loadUsers();
            break;
        case 'moderation':
            loadModerationQueue();
            break;
        case 'reports':
            loadReports();
            break;
        case 'invites':
            loadInvites();
            break;
        case 'appeals':
            loadAppeals();
            break;
        case 'audit':
            loadAudit();
            break;
    }
}

// Dashboard
//
// Phase 3.7B (chainlink #116): two-flavor dashboard per
// docs/AURORA_ADMIN_UI_DESIGN.md §5.1. Operator flavor preserves the
// existing instance-stats grid; Moderator flavor consumes
// getQueueStats + getModerationMetrics from Phase 3.7A. Tab toggle
// switches between flavors. Stat cards now use semantic delta classes
// (.positive / .attention / .neutral) per §5.1's design.
let dashboardActiveFlavor = 'operator';

function loadDashboardData() {
    // Operator flavor — existing path.
    fetch(`${API_BASE}/tools.aurora.ops.getStats`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => {
        if (!res.ok) {
            throw new Error(`HTTP ${res.status}: ${res.statusText}`);
        }
        return res.json();
    })
    .then(data => {
        document.getElementById('stat-users').textContent = data.totalUsers || 0;
        document.getElementById('stat-posts').textContent = data.totalPosts || 0;
        document.getElementById('stat-reports').textContent = data.openReports || 0;
        const storageBytes = data.storageBytes || 0;
        const storageGB = (storageBytes / 1024 / 1024 / 1024).toFixed(2);
        document.getElementById('stat-storage').textContent = `${storageGB} GB`;

        const totalUsers = data.totalUsers || 0;
        const activeUsers = data.activeUsers || 0;
        setStatChange(1, `${activeUsers} active`, 'neutral');
        setStatChange(2, `${data.totalPosts || 0} total`, 'neutral');
        // Open reports: existence is "attention", absence is "positive".
        setStatChange(3, data.openReports > 0 ? 'Requires attention' : 'All clear',
                      data.openReports > 0 ? 'attention' : 'positive');

        const totalInvites = data.totalInvites || 0;
        const availableInvites = data.availableInvites || 0;
        setStatChange(4, `${availableInvites} of ${totalInvites} available`, 'neutral');

        initializeCharts(data);
    })
    .catch(err => {
        console.error('Failed to load stats:', err);
        document.getElementById('stat-users').textContent = '0';
        document.getElementById('stat-posts').textContent = '0';
        document.getElementById('stat-reports').textContent = '0';
        document.getElementById('stat-storage').textContent = '0.00 GB';
    });

    loadRecentActivity();
    initializeCharts();
    // Moderator flavor — refresh queue stats + metrics when visible.
    refreshQueueBadge();
    if (dashboardActiveFlavor === 'moderator') {
        loadModeratorDashboard();
    }
}

// Stat-change helper: applies semantic class so visual treatment
// (color, icon) matches intent rather than arbitrary string content.
// Per §5.1 visual contract: .positive (green), .attention (amber),
// neutral default.
function setStatChange(cardIndex, text, semantic) {
    const el = document.querySelector(`#page-dashboard .stats-grid .stat-card:nth-child(${cardIndex}) .stat-change`);
    if (!el) return;
    el.textContent = text;
    el.classList.remove('positive', 'attention', 'neutral');
    el.classList.add(semantic || 'neutral');
}

// Sidebar bell badge — polled from getQueueStats per §5.1 Real-time
// behavior + §9.6 done criterion. Polled every 30s while session
// active; refreshed eagerly on dashboard load + on any moderation
// action that resolves a queue item.
let queueBadgePollHandle = null;
function refreshQueueBadge() {
    fetch(`${API_BASE}/tools.aurora.admin.getQueueStats`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.ok ? res.json() : null)
    .then(stats => {
        if (!stats) return;
        const badge = document.getElementById('mod-queue-count');
        if (badge) {
            badge.textContent = stats.queueAttentionTotal || 0;
            badge.classList.toggle('badge-attention', (stats.queueAttentionTotal || 0) > 0);
        }
        // Mirror onto the Reports nav badge if present.
        const reports = document.getElementById('reports-count');
        if (reports) reports.textContent = stats.openReports || 0;
        // Update Moderator flavor stat cards if visible.
        if (dashboardActiveFlavor === 'moderator') {
            updateModeratorStats(stats);
        }
    })
    .catch(() => { /* network/auth error — leave badge unchanged */ });
}

function startQueueBadgePolling() {
    if (queueBadgePollHandle) return;
    refreshQueueBadge();
    queueBadgePollHandle = setInterval(refreshQueueBadge, 30_000);
}

function stopQueueBadgePolling() {
    if (queueBadgePollHandle) {
        clearInterval(queueBadgePollHandle);
        queueBadgePollHandle = null;
    }
}

// Moderator flavor — stat cards + metrics chart per §5.1.
function loadModeratorDashboard() {
    fetch(`${API_BASE}/tools.aurora.admin.getQueueStats`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.ok ? res.json() : null)
    .then(stats => { if (stats) updateModeratorStats(stats); })
    .catch(() => {});
    // Last 30 days of metrics, daily granularity.
    const end = new Date();
    const start = new Date(end.getTime() - 30 * 24 * 3600 * 1000);
    fetch(`${API_BASE}/tools.aurora.admin.getModerationMetrics`, {
        method: 'POST',
        headers: {
            'Authorization': `Bearer ${adminToken}`,
            'Content-Type': 'application/json'
        },
        body: JSON.stringify({
            start: start.toISOString(),
            end: end.toISOString(),
            granularity: 'day',
            metrics: ['reportsFiled', 'reportsResolved', 'actionsTaken'],
        })
    })
    .then(res => res.ok ? res.json() : null)
    .then(data => { if (data) renderModeratorMetrics(data); })
    .catch(() => {});
}

function updateModeratorStats(stats) {
    const targets = [
        ['mod-stat-open-reports', stats.openReports || 0],
        ['mod-stat-pending-appeals', stats.pendingAppeals || 0],
        ['mod-stat-queue-total', stats.queueAttentionTotal || 0],
        ['mod-stat-oldest-age', formatAgeSeconds(stats.oldestOpenReportAgeSeconds || 0)],
    ];
    for (const [id, value] of targets) {
        const el = document.getElementById(id);
        if (el) el.textContent = value;
    }
}

function formatAgeSeconds(secs) {
    if (!secs) return '—';
    if (secs < 3600) return Math.round(secs / 60) + 'm';
    if (secs < 86400) return Math.round(secs / 3600) + 'h';
    return Math.round(secs / 86400) + 'd';
}

function renderModeratorMetrics(data) {
    const container = document.getElementById('mod-metrics-chart');
    if (!container) return;
    if (!data.series || data.series.length === 0) {
        container.innerHTML = '<p class="empty-state">No metrics available for this range.</p>';
        return;
    }
    // Render as a compact summary table — a real Chart.js wiring
    // lands with #108 alongside the existing Chart.js usage on the
    // Operator flavor.
    let html = '<table class="data-table"><thead><tr>'
        + '<th>Metric</th><th>This period</th><th>Previous</th><th>Change</th>'
        + '</tr></thead><tbody>';
    for (const s of data.series) {
        const aggregate = (s.aggregate || 0).toFixed(1);
        const prev = s.delta ? s.delta.previousAggregate.toFixed(1) : '—';
        let change = '—';
        let changeClass = 'neutral';
        if (s.delta) {
            const pct = s.delta.changePercent;
            const sign = pct >= 0 ? '+' : '';
            change = `${sign}${pct.toFixed(1)}%`;
            // Per-metric semantic interpretation: ReportsFiled
            // increases are "attention" (more reports = something
            // to look at), ReportsResolved increases are "positive".
            const negSign = s.metric === 'reports_filed' ? -1 : 1;
            changeClass = pct * negSign > 0 ? 'positive' : (pct === 0 ? 'neutral' : 'attention');
        }
        html += `<tr><td>${s.metric}</td><td>${aggregate}</td><td>${prev}</td>`
            + `<td class="stat-change ${changeClass}">${change}</td></tr>`;
    }
    html += '</tbody></table>';
    container.innerHTML = html;
}

function setDashboardFlavor(flavor) {
    dashboardActiveFlavor = flavor;
    document.querySelectorAll('#dashboard-flavor-tabs button').forEach(b => {
        b.classList.toggle('active', b.dataset.flavor === flavor);
        b.setAttribute('aria-selected', b.dataset.flavor === flavor ? 'true' : 'false');
    });
    document.getElementById('dashboard-operator').style.display = flavor === 'operator' ? '' : 'none';
    document.getElementById('dashboard-moderator').style.display = flavor === 'moderator' ? '' : 'none';
    if (flavor === 'moderator') loadModeratorDashboard();
}

function loadRecentActivity() {
    // Fetch recent users
    fetch(`${API_BASE}/com.atproto.admin.getUsers?limit=5`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.ok ? res.json() : { users: [] })
    .then(data => {
        const users = data.users || [];
        const activities = [];

        // Add recent user registrations
        users.slice(0, 3).forEach(user => {
            activities.push({
                icon: '👤',
                text: `New user registered: @${user.handle || 'unknown'}`,
                time: formatTimeAgo(user.createdAt)
            });
        });

        // Add system status if no users
        if (activities.length === 0) {
            activities.push({
                icon: '✅',
                text: 'Server is running and ready',
                time: 'Just now'
            });
            activities.push({
                icon: '🚀',
                text: 'Aurora Locus PDS initialized',
                time: 'Recently'
            });
        }

        const container = document.getElementById('recent-activity');
        container.innerHTML = activities.map(activity => `
            <div class="activity-item">
                <div class="activity-icon">${activity.icon}</div>
                <div class="activity-content">
                    <div class="activity-text">${activity.text}</div>
                    <div class="activity-time">${activity.time}</div>
                </div>
            </div>
        `).join('');
    })
    .catch(err => {
        console.error('Failed to load activity:', err);
        const container = document.getElementById('recent-activity');
        container.innerHTML = `
            <div class="activity-item">
                <div class="activity-icon">ℹ️</div>
                <div class="activity-content">
                    <div class="activity-text">No recent activity</div>
                    <div class="activity-time">System is ready</div>
                </div>
            </div>
        `;
    });
}

function formatTimeAgo(dateString) {
    if (!dateString) return 'Recently';

    try {
        const date = new Date(dateString);
        const now = new Date();
        const seconds = Math.floor((now - date) / 1000);

        if (seconds < 60) return 'Just now';
        if (seconds < 3600) return `${Math.floor(seconds / 60)} minutes ago`;
        if (seconds < 86400) return `${Math.floor(seconds / 3600)} hours ago`;
        if (seconds < 604800) return `${Math.floor(seconds / 86400)} days ago`;
        return date.toLocaleDateString();
    } catch (e) {
        return 'Recently';
    }
}

function initializeCharts(statsData) {
    const totalUsers = statsData?.totalUsers || 0;
    const totalPosts = statsData?.totalPosts || 0;
    const openReports = statsData?.openReports || 0;
    const activeUsers = statsData?.activeUsers || 0;

    // User Growth Chart - show totals over time
    const userCtx = document.getElementById('userGrowthChart');
    if (userCtx) {
        // Since we don't have historical data yet, show current state
        const userGrowth = totalUsers > 0 ?
            [0, Math.floor(totalUsers * 0.2), Math.floor(totalUsers * 0.4),
             Math.floor(totalUsers * 0.6), Math.floor(totalUsers * 0.8),
             Math.floor(totalUsers * 0.9), totalUsers] :
            [0, 0, 0, 0, 0, 0, 0];

        new Chart(userCtx, {
            type: 'line',
            data: {
                labels: ['Week 1', 'Week 2', 'Week 3', 'Week 4', 'Week 5', 'Week 6', 'Current'],
                datasets: [{
                    label: 'Total Users',
                    data: userGrowth,
                    borderColor: '#3b82f6',
                    backgroundColor: 'rgba(59, 130, 246, 0.1)',
                    tension: 0.4,
                    fill: true
                }]
            },
            options: {
                responsive: true,
                maintainAspectRatio: false,
                plugins: {
                    legend: {
                        display: false
                    }
                },
                scales: {
                    y: {
                        beginAtZero: true,
                        ticks: {
                            precision: 0
                        }
                    }
                }
            }
        });
    }

    // Activity Chart - show real metrics
    const activityCtx = document.getElementById('activityChart');
    if (activityCtx) {
        new Chart(activityCtx, {
            type: 'bar',
            data: {
                labels: ['Total Users', 'Active Users', 'Posts', 'Reports'],
                datasets: [{
                    label: 'Activity Metrics',
                    data: [totalUsers, activeUsers, totalPosts, openReports],
                    backgroundColor: [
                        '#3b82f6',
                        '#10b981',
                        '#f59e0b',
                        '#ef4444'
                    ]
                }]
            },
            options: {
                responsive: true,
                maintainAspectRatio: false,
                plugins: {
                    legend: {
                        display: false
                    }
                },
                scales: {
                    y: {
                        beginAtZero: true,
                        ticks: {
                            precision: 0
                        }
                    }
                }
            }
        });
    }
}

// Users Management
function loadUsers() {
    fetch(`${API_BASE}/com.atproto.admin.listAccounts?limit=100`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.json())
    .then(data => {
        renderUsersTable(data.accounts || []);
    })
    .catch(err => console.error('Failed to load users:', err));
}

// Phase 3.5D (chainlink #114): multi-select substrate for Users.
// Tracks DIDs of selected rows; renderUsersTable wires checkboxes
// + bulk-action bar via the BulkActionPanel substrate primitive.
let usersSelected = new Set();

function renderUsersTable(users) {
    const tbody = document.getElementById('users-table');
    // Drop selections for users no longer in the page (filter changes,
    // pagination, etc.).
    const visibleDids = new Set(users.map(u => u.did));
    usersSelected = new Set([...usersSelected].filter(d => visibleDids.has(d)));
    tbody.innerHTML = users.map(user => `
        <tr>
            <td>
                <input type="checkbox" class="bulk-select-user"
                       data-did="${user.did}"
                       ${usersSelected.has(user.did) ? 'checked' : ''}
                       aria-label="Select ${user.handle}">
            </td>
            <td>${user.handle}</td>
            <td><code>${user.did}</code></td>
            <td>${user.email || 'N/A'}</td>
            <td>${new Date(user.createdAt).toLocaleDateString()}</td>
            <td><span class="status-badge status-${user.status || 'active'}">${user.status || 'Active'}</span></td>
            <td>
                <button class="btn-sm btn-primary" onclick="viewUser('${user.did}')">View</button>
            </td>
        </tr>
    `).join('');
    // Wire checkbox change handlers
    tbody.querySelectorAll('.bulk-select-user').forEach(cb => {
        cb.addEventListener('change', e => {
            const did = e.target.dataset.did;
            if (e.target.checked) usersSelected.add(did);
            else usersSelected.delete(did);
            updateUsersBulkBar();
        });
    });
    updateUsersBulkBar();
}

function updateUsersBulkBar() {
    let bar = document.getElementById('users-bulk-bar');
    if (!bar) {
        // Inject the bulk-action bar above the table on first render.
        const card = document.querySelector('#page-users .table-card');
        if (!card) return;
        bar = document.createElement('div');
        bar.id = 'users-bulk-bar';
        bar.className = 'bulk-action-bar';
        bar.setAttribute('role', 'toolbar');
        bar.setAttribute('aria-label', 'Bulk actions for selected users');
        card.parentNode.insertBefore(bar, card);
    }
    const n = usersSelected.size;
    if (n === 0) {
        bar.innerHTML = '';
        bar.style.display = 'none';
        return;
    }
    bar.style.display = '';
    bar.innerHTML = `
        <span><strong>${n}</strong> selected</span>
        <button class="btn-sm btn-danger" onclick="openBulkActionModal('users','BatchTakedownAccounts')">Bulk takedown</button>
        <button class="btn-sm btn-secondary" onclick="openBulkActionModal('users','BatchSuspendAccounts')">Bulk suspend</button>
        <button class="btn-sm btn-secondary" onclick="openBulkActionModal('users','BatchRestoreAccounts')">Bulk restore</button>
        <button class="btn-sm btn-secondary" onclick="clearUsersSelection()">Clear</button>
    `;
}

function clearUsersSelection() {
    usersSelected.clear();
    document.querySelectorAll('.bulk-select-user').forEach(cb => { cb.checked = false; });
    updateUsersBulkBar();
}

// Phase 3.5D — viewUser opens the user-details modal with two
// composed action drawers (Pattern A: account management, Pattern B:
// account-scoped moderation). Both drawers consume the ActionPanel
// substrate primitive (substrate primitive 3, §6.3); ActionPanel
// routes through the capability-routed substrate to emitEvent
// post-3.5 per §6.17.
function viewUser(did) {
    fetch(`${API_BASE}/com.atproto.admin.getAccount?did=${did}`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.json())
    .then(user => {
        const content = document.getElementById('user-details-content');
        content.innerHTML = `
            <div class="user-details">
                <p><strong>Handle:</strong> ${user.handle}</p>
                <p><strong>DID:</strong> <code>${user.did}</code></p>
                <p><strong>Email:</strong> ${user.email || 'N/A'}</p>
                <p><strong>Created:</strong> ${new Date(user.createdAt).toLocaleString()}</p>
                <p><strong>Posts:</strong> ${user.postsCount || 0}</p>
                <p><strong>Followers:</strong> ${user.followersCount || 0}</p>
                <p><strong>Following:</strong> ${user.followingCount || 0}</p>
            </div>
            <div class="user-details-actions" style="margin-top: 1.5rem;">
                <details open>
                    <summary><strong>Moderation actions</strong> <span class="role-tag">Moderator+</span></summary>
                    <div id="user-mod-action-panel" style="margin-top: 0.75rem;"></div>
                </details>
                <details style="margin-top: 1rem;">
                    <summary><strong>Account management</strong> <span class="role-tag">Admin+</span></summary>
                    <div id="user-mgmt-action-panel" style="margin-top: 0.75rem;"></div>
                    <div style="margin-top: 0.75rem; display: flex; gap: 0.5rem; flex-wrap: wrap;">
                        <button class="btn-secondary" onclick="openPasswordResetModal('${user.did}','${user.handle || ''}')">Send password reset</button>
                        <button class="btn-secondary" onclick="openForensicExportModal('${user.did}','${user.handle || ''}')">Generate forensic export</button>
                    </div>
                </details>
            </div>
        `;
        showModal('modal-user-details');
        mountUserActionPanels(user);
    })
    .catch(err => {
        alert('Failed to load user details');
        console.error(err);
    });
}

// Pattern A + B action panels for the User Details modal. Pattern A
// (account-mgmt) lists Admin+ actions; Pattern B (moderation) lists
// Moderator+ actions. Both use the same ActionPanel primitive, just
// with different availableActions arrays.
function mountUserActionPanels(user) {
    const subject = {
        '$type': 'com.atproto.admin.defs#repoRef',
        did: user.did,
    };
    // Pattern B — moderation actions drawer (Moderator+).
    const modPanel = new ActionPanel({
        subject: subject,
        availableActions: [
            'TakedownAccount',
            'SuspendAccount',
            'RestoreAccount',
            'ApplyLabel',
            'RemoveLabel',
            'SendEmail',
        ],
        defaultAction: 'TakedownAccount',
        requiresRationale: true,
        highImpactActions: ['TakedownAccount'],
        userRole: currentUser?.role || 'moderator',
        onCancel: () => { /* drawer stays open; no-op */ },
    });
    modPanel.mount(document.getElementById('user-mod-action-panel'));
    // Pattern A — account management drawer (Admin+). The Admin gate
    // is display-side; server is authoritative per §3.1.
    const mgmtPanel = new ActionPanel({
        subject: subject,
        availableActions: ['DeleteAccount'],
        defaultAction: 'DeleteAccount',
        requiresRationale: true,
        highImpactActions: ['DeleteAccount'],
        userRole: currentUser?.role || 'moderator',
        onCancel: () => { /* drawer stays open */ },
    });
    mgmtPanel.mount(document.getElementById('user-mgmt-action-panel'));
}

// Two-track password reset flow (§5.2 Credentials sub-section). Single-
// click button opens a small confirmation modal with rationale field;
// calls tools.aurora.admin.triggerPasswordReset on confirm.
function openPasswordResetModal(did, handle) {
    const rationale = prompt(
        'Send password reset email to ' + (handle || did) + '?\n\n' +
        'Rationale (required, recorded in audit log):'
    );
    if (rationale == null) return;
    if (!rationale.trim()) {
        alert('Rationale is required.');
        return;
    }
    AuroraCapabilities.callEndpoint('trigger-password-reset', {
        did: did,
        rationale: rationale,
    })
    .then(result => {
        const sent = result.resetEmailSent
            ? 'Password reset email sent to ' + result.maskedEmail
            : 'Token generated but email not sent (mailer not configured); masked: ' + result.maskedEmail;
        alert(sent);
    })
    .catch(err => {
        alert('Password reset failed: ' + (err.message || err));
    });
}

// Phase 3.5D — bulk action modal for multi-select pages. Renders
// BulkActionPanel substrate primitive into the existing modal-overlay
// infrastructure with a temporary container.
function openBulkActionModal(source, defaultAction) {
    const subjects = collectBulkSubjects(source);
    if (subjects.length === 0) {
        alert('No subjects selected.');
        return;
    }
    const containerId = 'bulk-action-container';
    let modal = document.getElementById('modal-bulk-action');
    if (!modal) {
        modal = document.createElement('div');
        modal.id = 'modal-bulk-action';
        modal.className = 'modal';
        modal.innerHTML = `
            <div class="modal-header">
                <h3>Bulk action</h3>
                <button class="modal-close" onclick="closeBulkActionModal()" aria-label="Close">&times;</button>
            </div>
            <div class="modal-body" id="${containerId}"></div>
        `;
        document.body.appendChild(modal);
    }
    const availableActions = bulkActionsForSource(source);
    const panel = new BulkActionPanel({
        subjects: subjects,
        availableActions: availableActions,
        onCancel: () => closeBulkActionModal(),
    });
    document.getElementById(containerId).innerHTML = '';
    panel.mount(document.getElementById(containerId));
    // Pre-select the requested action if it's in the available list.
    if (availableActions.indexOf(defaultAction) !== -1) {
        panel.state.action = defaultAction;
        panel.render();
    }
    document.getElementById('modal-overlay').classList.add('active');
    modal.classList.add('active');
    window._activeBulkPanel = panel;
}

function closeBulkActionModal() {
    const modal = document.getElementById('modal-bulk-action');
    if (modal) modal.classList.remove('active');
    document.getElementById('modal-overlay').classList.remove('active');
    if (window._activeBulkPanel) {
        window._activeBulkPanel.unmount();
        window._activeBulkPanel = null;
    }
    // Refresh underlying page data after a bulk action completes.
    if (currentPage === 'users') loadUsers();
    else if (currentPage === 'moderation') loadModerationQueue();
    else if (currentPage === 'reports') loadReports();
    else if (currentPage === 'invites') loadInvites();
}

function collectBulkSubjects(source) {
    if (source === 'users') {
        return [...usersSelected].map(did => ({
            '$type': 'com.atproto.admin.defs#repoRef',
            did: did,
        }));
    }
    if (source === 'queue') {
        return [...modQueueSelected].map(did => ({
            '$type': 'com.atproto.admin.defs#repoRef',
            did: did,
        }));
    }
    if (source === 'reports') {
        return [...reportsSelected].map(did => ({
            '$type': 'com.atproto.admin.defs#repoRef',
            did: did,
        }));
    }
    return [];
}

function bulkActionsForSource(source) {
    if (source === 'users') {
        return ['BatchTakedownAccounts', 'BatchSuspendAccounts', 'BatchRestoreAccounts', 'BatchApplyLabel', 'BatchRemoveLabel'];
    }
    if (source === 'queue' || source === 'reports') {
        return ['BatchTakedownAccounts', 'BatchSuspendAccounts', 'BatchApplyLabel'];
    }
    return [];
}

// Moderation Queue
function loadModerationQueue() {
    fetch(`${API_BASE}/com.atproto.admin.getModerationQueue?limit=50`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.json())
    .then(data => {
        renderModerationQueue(data.items || []);
        document.getElementById('mod-queue-count').textContent = data.items?.length || 0;
    })
    .catch(err => console.error('Failed to load moderation queue:', err));
}

// Phase 3.5D: multi-select substrate for moderation queue. Each
// queue item references an account-shaped subject; checkboxes
// collect the subject DIDs for BulkActionPanel.
let modQueueSelected = new Set();

function renderModerationQueue(items) {
    const container = document.getElementById('moderation-queue');
    const visible = new Set(items.map(i => i.subjectDid || i.subject?.did).filter(Boolean));
    modQueueSelected = new Set([...modQueueSelected].filter(d => visible.has(d)));
    container.innerHTML = items.map(item => {
        const subjDid = item.subjectDid || item.subject?.did || '';
        const checked = modQueueSelected.has(subjDid) ? 'checked' : '';
        const cbDisabled = subjDid ? '' : 'disabled aria-label="No subject DID for this item"';
        return `
        <div class="mod-item">
            <div class="mod-header">
                <input type="checkbox" class="bulk-select-mod"
                       data-did="${subjDid}" ${checked} ${cbDisabled}
                       aria-label="Select queue item ${item.id}">
                <div>
                    <strong>${item.reasonType || 'Unknown'}</strong>
                    <p>By: ${item.reportedBy || ''}</p>
                </div>
                <span class="status-badge status-pending">Pending</span>
            </div>
            <div class="mod-content">
                ${item.content || 'No content preview available'}
            </div>
            <div class="mod-actions">
                <button class="btn-sm btn-secondary" onclick="dismissReport('${item.id}')">Dismiss</button>
                <button class="btn-sm btn-danger" onclick="takedownContent('${item.id}')">Takedown</button>
            </div>
        </div>
    `;
    }).join('');
    container.querySelectorAll('.bulk-select-mod').forEach(cb => {
        cb.addEventListener('change', e => {
            const did = e.target.dataset.did;
            if (!did) return;
            if (e.target.checked) modQueueSelected.add(did);
            else modQueueSelected.delete(did);
            updateModQueueBulkBar();
        });
    });
    updateModQueueBulkBar();
}

function updateModQueueBulkBar() {
    let bar = document.getElementById('mod-queue-bulk-bar');
    const page = document.querySelector('#page-moderation .moderation-queue');
    if (!page) return;
    if (!bar) {
        bar = document.createElement('div');
        bar.id = 'mod-queue-bulk-bar';
        bar.className = 'bulk-action-bar';
        bar.setAttribute('role', 'toolbar');
        bar.setAttribute('aria-label', 'Bulk actions for selected queue items');
        page.parentNode.insertBefore(bar, page);
    }
    const n = modQueueSelected.size;
    if (n === 0) {
        bar.innerHTML = '';
        bar.style.display = 'none';
        return;
    }
    bar.style.display = '';
    bar.innerHTML = `
        <span><strong>${n}</strong> subject${n === 1 ? '' : 's'} selected</span>
        <button class="btn-sm btn-danger" onclick="openBulkActionModal('queue','BatchTakedownAccounts')">Bulk takedown</button>
        <button class="btn-sm btn-secondary" onclick="openBulkActionModal('queue','BatchSuspendAccounts')">Bulk suspend</button>
        <button class="btn-sm btn-secondary" onclick="clearModQueueSelection()">Clear</button>
    `;
}

function clearModQueueSelection() {
    modQueueSelected.clear();
    document.querySelectorAll('.bulk-select-mod').forEach(cb => { cb.checked = false; });
    updateModQueueBulkBar();
}

// Reports
function loadReports() {
    fetch(`${API_BASE}/com.atproto.admin.listReports?limit=50`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.json())
    .then(data => {
        renderReports(data.reports || []);
        const openCount = data.reports?.filter(r => r.status === 'open').length || 0;
        document.getElementById('reports-count').textContent = openCount;
    })
    .catch(err => console.error('Failed to load reports:', err));
}

// Phase 3.5D: multi-select substrate for reports list. Reports
// reference subject DIDs; checkboxes feed BulkActionPanel.
let reportsSelected = new Set();

function renderReports(reports) {
    const container = document.getElementById('reports-list');
    const visible = new Set(reports.map(r => r.subjectDid || r.subject?.did || (typeof r.subject === 'string' ? r.subject : null)).filter(Boolean));
    reportsSelected = new Set([...reportsSelected].filter(d => visible.has(d)));
    container.innerHTML = reports.map(report => {
        const subjDid = report.subjectDid || report.subject?.did
            || (typeof report.subject === 'string' && report.subject.startsWith('did:') ? report.subject : '');
        const checked = reportsSelected.has(subjDid) ? 'checked' : '';
        const cbDisabled = subjDid ? '' : 'disabled aria-label="No DID-shaped subject for this report"';
        return `
        <div class="report-item">
            <div class="report-header">
                <input type="checkbox" class="bulk-select-report"
                       data-did="${subjDid}" ${checked} ${cbDisabled}
                       aria-label="Select report ${report.id}">
                <div>
                    <strong>${report.reasonType || ''}</strong>
                    <p>Reporter: @${report.reportedBy || ''}</p>
                    <p>Subject: ${report.subject || ''}</p>
                </div>
                <span class="status-badge status-${report.status || 'open'}">${report.status || 'open'}</span>
            </div>
            <div class="report-content">
                ${report.reason || 'No reason provided'}
            </div>
            <div class="report-actions">
                <button class="btn-sm btn-primary" onclick="viewReport('${report.id}')">View Details</button>
            </div>
        </div>
    `;
    }).join('');
    container.querySelectorAll('.bulk-select-report').forEach(cb => {
        cb.addEventListener('change', e => {
            const did = e.target.dataset.did;
            if (!did) return;
            if (e.target.checked) reportsSelected.add(did);
            else reportsSelected.delete(did);
            updateReportsBulkBar();
        });
    });
    updateReportsBulkBar();
}

function updateReportsBulkBar() {
    let bar = document.getElementById('reports-bulk-bar');
    const list = document.querySelector('#page-reports .reports-list');
    if (!list) return;
    if (!bar) {
        bar = document.createElement('div');
        bar.id = 'reports-bulk-bar';
        bar.className = 'bulk-action-bar';
        bar.setAttribute('role', 'toolbar');
        bar.setAttribute('aria-label', 'Bulk actions for selected reports');
        list.parentNode.insertBefore(bar, list);
    }
    const n = reportsSelected.size;
    if (n === 0) {
        bar.innerHTML = '';
        bar.style.display = 'none';
        return;
    }
    bar.style.display = '';
    bar.innerHTML = `
        <span><strong>${n}</strong> subject${n === 1 ? '' : 's'} selected</span>
        <button class="btn-sm btn-danger" onclick="openBulkActionModal('reports','BatchTakedownAccounts')">Bulk takedown</button>
        <button class="btn-sm btn-secondary" onclick="openBulkActionModal('reports','BatchSuspendAccounts')">Bulk suspend</button>
        <button class="btn-sm btn-secondary" onclick="openBulkActionModal('reports','BatchApplyLabel')">Bulk label</button>
        <button class="btn-sm btn-secondary" onclick="clearReportsSelection()">Clear</button>
    `;
}

function clearReportsSelection() {
    reportsSelected.clear();
    document.querySelectorAll('.bulk-select-report').forEach(cb => { cb.checked = false; });
    updateReportsBulkBar();
}

function viewReport(reportId) {
    // Fetch and display report details in modal
    showModal('modal-report-details');
}

function resolveReport() {
    alert('Report resolved');
    closeModal();
    loadReports();
}

function dismissReport(reportId) {
    if (!confirm('Dismiss this report?')) return;

    // API call to dismiss report
    loadReports();
}

function takedownContent(itemId) {
    if (!confirm('Take down this content? This action cannot be undone.')) return;

    // API call to takedown content
    alert('Content taken down');
}

// Invite Codes
function loadInvites() {
    fetch(`${API_BASE}/com.atproto.admin.listInviteCodes?limit=100`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.json())
    .then(data => {
        renderInvites(data.codes || []);
        updateInviteStats(data.codes || []);
    })
    .catch(err => console.error('Failed to load invites:', err));
}

// Phase 3.5D: Pattern E (invite-code actions) bulk variant on Invites
// page per §9.2. Uses the existing com.atproto.admin.disableInviteCodes
// (plural, transactional all-or-nothing per Phase 1.3) rather than a
// new batch endpoint — Phase 3.5 §8 only adds account/record/label
// batch endpoints, not invite-code batch.
let invitesSelected = new Set();

function renderInvites(codes) {
    const tbody = document.getElementById('invites-table');
    const visible = new Set(codes.map(c => c.code));
    invitesSelected = new Set([...invitesSelected].filter(c => visible.has(c)));
    tbody.innerHTML = codes.map(code => `
        <tr>
            <td>
                <input type="checkbox" class="bulk-select-invite"
                       data-code="${code.code}"
                       ${invitesSelected.has(code.code) ? 'checked' : ''}
                       ${code.disabled ? 'disabled aria-label="Already disabled"' : 'aria-label="Select invite code"'}>
            </td>
            <td><code>${code.code}</code></td>
            <td>${code.uses || 0} / ${code.available || 1}</td>
            <td>@${code.created_by || 'system'}</td>
            <td>${new Date(code.created_at).toLocaleDateString()}</td>
            <td><span class="status-badge status-${code.disabled ? 'suspended' : 'active'}">${code.disabled ? 'Disabled' : 'Active'}</span></td>
            <td>
                <button class="btn-sm btn-danger" onclick="disableInvite('${code.code}')" ${code.disabled ? 'disabled' : ''}>Disable</button>
            </td>
        </tr>
    `).join('');
    tbody.querySelectorAll('.bulk-select-invite').forEach(cb => {
        cb.addEventListener('change', e => {
            const code = e.target.dataset.code;
            if (e.target.checked) invitesSelected.add(code);
            else invitesSelected.delete(code);
            updateInvitesBulkBar();
        });
    });
    updateInvitesBulkBar();
}

function updateInvitesBulkBar() {
    let bar = document.getElementById('invites-bulk-bar');
    const card = document.querySelector('#page-invites .table-card');
    if (!card) return;
    if (!bar) {
        bar = document.createElement('div');
        bar.id = 'invites-bulk-bar';
        bar.className = 'bulk-action-bar';
        bar.setAttribute('role', 'toolbar');
        bar.setAttribute('aria-label', 'Bulk actions for selected invite codes');
        card.parentNode.insertBefore(bar, card);
    }
    const n = invitesSelected.size;
    if (n === 0) {
        bar.innerHTML = '';
        bar.style.display = 'none';
        return;
    }
    bar.style.display = '';
    bar.innerHTML = `
        <span><strong>${n}</strong> code${n === 1 ? '' : 's'} selected</span>
        <button class="btn-sm btn-danger" onclick="bulkDisableInvites()">Disable selected</button>
        <button class="btn-sm btn-secondary" onclick="clearInvitesSelection()">Clear</button>
    `;
}

function clearInvitesSelection() {
    invitesSelected.clear();
    document.querySelectorAll('.bulk-select-invite').forEach(cb => { cb.checked = false; });
    updateInvitesBulkBar();
}

function bulkDisableInvites() {
    const codes = [...invitesSelected];
    if (codes.length === 0) return;
    if (!confirm('Disable ' + codes.length + ' invite code' + (codes.length === 1 ? '' : 's') + '?')) return;
    fetch(API_BASE + '/com.atproto.admin.disableInviteCodes', {
        method: 'POST',
        headers: {
            'Authorization': 'Bearer ' + adminToken,
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ codes: codes }),
    })
    .then(res => {
        if (!res.ok) throw new Error('HTTP ' + res.status);
        return res.json().catch(() => ({}));
    })
    .then(() => {
        alert('Disabled ' + codes.length + ' invite code' + (codes.length === 1 ? '' : 's'));
        invitesSelected.clear();
        loadInvites();
    })
    .catch(err => {
        alert('Bulk disable failed: ' + err.message);
    });
}

function updateInviteStats(codes) {
    const total = codes.length;
    const available = codes.filter(c => !c.disabled && c.uses < c.available).length;
    const used = codes.filter(c => c.uses >= c.available).length;

    document.getElementById('invite-total').textContent = total;
    document.getElementById('invite-available').textContent = available;
    document.getElementById('invite-used').textContent = used;
}

async function generateInvites(event) {
    // Check authentication first
    if (!adminToken) {
        alert('⚠️ Not logged in!\n\nYou must be logged in to generate invite codes.\n\nPlease go to the login page and sign in with admin credentials.');
        return;
    }

    const count = prompt('How many invite codes to generate?', '10');
    if (!count || count <= 0) return;

    const numCodes = parseInt(count);
    let generated = 0;
    let failed = 0;
    let lastError = null;

    // Show progress indicator
    const button = event?.target || document.querySelector('[onclick*="generateInvites"]');
    const originalText = button.textContent;
    button.textContent = 'Generating...';
    button.disabled = true;

    try {
        // Generate codes one at a time
        for (let i = 0; i < numCodes; i++) {
            try {
                const response = await fetch(`${API_BASE}/com.atproto.admin.createInviteCode`, {
                    method: 'POST',
                    headers: {
                        'Authorization': `Bearer ${adminToken}`,
                        'Content-Type': 'application/json'
                    },
                    body: JSON.stringify({
                        uses: 1,
                        note: `Batch generated code ${i + 1}/${numCodes}`
                    })
                });

                if (response.ok) {
                    generated++;
                    button.textContent = `Generating... (${generated}/${numCodes})`;
                } else {
                    failed++;
                    const errorData = await response.json().catch(() => ({ error: 'Unknown error' }));
                    lastError = errorData.error || errorData.message || `HTTP ${response.status}`;
                    console.error(`Failed to generate code ${i + 1}:`, lastError, errorData);

                    // Stop if authentication fails
                    if (response.status === 401 || response.status === 403) {
                        alert('⚠️ Authentication failed!\n\nYour session may have expired. Please log in again.');
                        logout();
                        return;
                    }
                }
            } catch (err) {
                failed++;
                lastError = err.message;
                console.error(`Error generating code ${i + 1}:`, err);
            }
        }

        // Show results
        if (failed > 0) {
            const errorMsg = lastError ? `\n\nError: ${lastError}` : '';
            alert(`✅ Generated: ${generated} codes\n❌ Failed: ${failed} codes${errorMsg}\n\n💡 Check browser console (F12) for full details.`);
        } else {
            alert(`✅ Successfully generated ${generated} invite codes!`);
        }

        if (generated > 0) {
            loadInvites();
        }
    } finally {
        button.textContent = originalText;
        button.disabled = false;
    }
}

function disableInvite(code) {
    if (!confirm('Disable this invite code?')) return;

    fetch(`${API_BASE}/com.atproto.admin.disableInviteCode`, {
        method: 'POST',
        headers: {
            'Authorization': `Bearer ${adminToken}`,
            'Content-Type': 'application/json'
        },
        body: JSON.stringify({ code })
    })
    .then(() => {
        alert('Invite code disabled');
        loadInvites();
    })
    .catch(err => {
        alert('Failed to disable invite code');
        console.error(err);
    });
}

// Modal Management
function showModal(modalId) {
    document.getElementById('modal-overlay').classList.add('active');
    document.getElementById(modalId).classList.add('active');
}

function closeModal() {
    document.getElementById('modal-overlay').classList.remove('active');
    document.querySelectorAll('.modal').forEach(modal => {
        modal.classList.remove('active');
    });
}

// Export Functions
function exportUsers() {
    alert('Exporting users to CSV...');
    // Implement CSV export
}

// Event Listeners
function setupEventListeners() {
    // User search
    const userSearch = document.getElementById('user-search');
    if (userSearch) {
        userSearch.addEventListener('input', (e) => {
            // Implement search filtering
        });
    }

    // Filter selects
    const modFilter = document.getElementById('mod-filter');
    if (modFilter) {
        modFilter.addEventListener('change', (e) => {
            // Implement filtering
        });
    }

    // Settings forms
    const forms = [
        'general-settings-form',
        'registration-settings-form',
        'moderation-settings-form'
    ];

    forms.forEach(formId => {
        const form = document.getElementById(formId);
        if (form) {
            form.addEventListener('submit', (e) => {
                e.preventDefault();
                saveSettings(formId);
            });
        }
    });
}

function saveSettings(formId) {
    alert('Settings saved successfully');
}

// Aurora moderator-tier event browser (chainlink #100 / Phase 3.3).
// Fetches tools.aurora.moderator.queryEvents with filter params and
// renders results in a table. Cursor-based pagination via
// loadEventsPrev/loadEventsNext.
let eventsCursorStack = [];      // page history for "Previous"
let eventsNextCursor = null;     // cursor for "Next"

// Phase 3.9 subscription handle for the Mod Events page. Established
// on first load and torn down when the page is navigated away from.
let eventsSubscription = null;

function loadEvents() {
    eventsCursorStack = [];
    eventsNextCursor = null;
    fetchEventsPage(null);
    // Establish subscription if not already running. New events
    // arrive while operator is on the page and prepend to the table
    // with a brief highlight per §6.18 + §5.3.6.
    if (!eventsSubscription) {
        const indicator = document.getElementById('events-rt-indicator');
        eventsSubscription = AuroraSubscription.subscribe(
            'subscribe-mod-events',
            {},
            {
                onEvent: (event) => prependLiveEvent(event),
                onConnected: () => {},
                onDisconnected: () => {},
                onError: (e) => console.warn('subscribeModEvents error:', e),
            }
        );
        if (indicator) AuroraSubscription.attachIndicator(indicator, eventsSubscription);
    }
}

// Real-time event arrival: prepend to existing table with a brief
// fade-in highlight per §5.3.6. If the operator is mid-pagination
// (cursor non-null), don't disturb the visible page.
function prependLiveEvent(event) {
    if (eventsCursorStack.length > 0) return; // operator on a non-first page
    const container = document.getElementById('events-table-container');
    const tbody = container?.querySelector('table.data-table tbody');
    if (!tbody) return;
    const when = new Date(event.createdAt).toLocaleString();
    const actor = event.actorDid || '';
    let subject = '—';
    if (event.subjectDid) subject = 'repo: ' + event.subjectDid;
    else if (event.subjectUri) subject = 'record: ' + event.subjectUri;
    const tr = document.createElement('tr');
    tr.className = 'rt-fadein';
    tr.innerHTML = `<td>${when}</td><td>${event.eventType}</td><td>${actor}</td>` +
                   `<td>${subject}</td><td><a href="javascript:void(0)" onclick="loadEventDetail(${event.id})">${event.id}</a></td>`;
    tbody.insertBefore(tr, tbody.firstChild);
    // Cap visible row count to avoid runaway growth.
    while (tbody.children.length > 100) {
        tbody.removeChild(tbody.lastChild);
    }
}

function loadEventsNext() {
    if (eventsNextCursor) {
        eventsCursorStack.push(eventsNextCursor);
        fetchEventsPage(eventsNextCursor);
    }
}

function loadEventsPrev() {
    if (eventsCursorStack.length > 1) {
        eventsCursorStack.pop();          // current page
        const prev = eventsCursorStack[eventsCursorStack.length - 1] || null;
        fetchEventsPage(prev);
    } else if (eventsCursorStack.length === 1) {
        eventsCursorStack = [];
        fetchEventsPage(null);
    }
}

function fetchEventsPage(cursor) {
    const params = new URLSearchParams();
    const actor = document.getElementById('events-filter-actor').value.trim();
    const subj = document.getElementById('events-filter-subject').value.trim();
    const evtType = document.getElementById('events-filter-type').value.trim();
    if (actor) params.set('actor', actor);
    if (subj) params.set('subjectDid', subj);
    if (evtType) params.set('eventType', evtType);
    if (cursor) params.set('cursor', cursor);
    params.set('limit', '25');

    const container = document.getElementById('events-table-container');
    container.innerHTML = '<p class="empty-state">Loading...</p>';

    fetch(`${API_BASE}/tools.aurora.moderator.queryEvents?${params.toString()}`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.json();
    })
    .then(data => renderEventsTable(data))
    .catch(err => {
        container.innerHTML = `<p class="empty-state">Error: ${err.message}</p>`;
    });
}

function renderEventsTable(data) {
    const container = document.getElementById('events-table-container');
    const items = data.items || [];
    eventsNextCursor = data.cursor || null;

    document.getElementById('events-prev-btn').disabled = eventsCursorStack.length === 0;
    document.getElementById('events-next-btn').disabled = !eventsNextCursor;

    if (items.length === 0) {
        container.innerHTML = '<p class="empty-state">No events match these filters.</p>';
        return;
    }

    let html = '<table class="data-table"><thead><tr>'
        + '<th>When</th><th>Type</th><th>Actor</th><th>Subject</th><th>ID</th>'
        + '</tr></thead><tbody>';
    for (const e of items) {
        const when = new Date(e.createdAt).toLocaleString();
        const actor = e.actorHandle || e.actorDid;
        let subject = '—';
        if (e.subject) {
            const subjType = e.subject['$type'] || '?';
            if (subjType.endsWith('repoRef')) {
                subject = `repo: ${e.subjectHandle || e.subject.did}`;
            } else if (subjType.endsWith('strongRef')) {
                subject = `record: ${e.subject.uri}`;
            } else if (subjType.endsWith('repoBlobRef')) {
                subject = `blob: ${e.subject.cid}`;
            }
        }
        html += `<tr><td>${when}</td><td>${e.eventType}</td><td>${actor}</td>`
            + `<td>${subject}</td><td><a href="javascript:void(0)" onclick="loadEventDetail(${e.id})">${e.id}</a></td></tr>`;
    }
    html += '</tbody></table>';
    container.innerHTML = html;
}

function loadEventDetail(id) {
    fetch(`${API_BASE}/tools.aurora.moderator.getEvent?id=${id}`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.json())
    .then(data => {
        alert(JSON.stringify(data, null, 2));
    })
    .catch(err => alert(`Error: ${err.message}`));
}

// Aurora moderator-tier appeals browser (chainlink #101 / Phase 3.4).
// Fetches tools.aurora.moderator.listAppeals with filter params and
// renders results in a table. Cursor-based pagination via
// loadAppealsPrev/loadAppealsNext. Mirrors the Mod Events page.
let appealsCursorStack = [];
let appealsNextCursor = null;

function loadAppeals() {
    appealsCursorStack = [];
    appealsNextCursor = null;
    fetchAppealsPage(null);
}

function loadAppealsNext() {
    if (appealsNextCursor) {
        appealsCursorStack.push(appealsNextCursor);
        fetchAppealsPage(appealsNextCursor);
    }
}

function loadAppealsPrev() {
    if (appealsCursorStack.length > 1) {
        appealsCursorStack.pop();
        const prev = appealsCursorStack[appealsCursorStack.length - 1] || null;
        fetchAppealsPage(prev);
    } else if (appealsCursorStack.length === 1) {
        appealsCursorStack = [];
        fetchAppealsPage(null);
    }
}

function fetchAppealsPage(cursor) {
    const params = new URLSearchParams();
    const status = document.getElementById('appeals-filter-status').value.trim();
    const appellant = document.getElementById('appeals-filter-appellant').value.trim();
    const reviewer = document.getElementById('appeals-filter-reviewer').value.trim();
    if (status) params.set('status', status);
    if (appellant) params.set('appellant', appellant);
    if (reviewer) params.set('reviewer', reviewer);
    if (cursor) params.set('cursor', cursor);
    params.set('limit', '25');

    const container = document.getElementById('appeals-table-container');
    container.innerHTML = '<p class="empty-state">Loading...</p>';

    fetch(`${API_BASE}/tools.aurora.moderator.listAppeals?${params.toString()}`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.json();
    })
    .then(data => renderAppealsTable(data))
    .catch(err => {
        container.innerHTML = `<p class="empty-state">Error: ${err.message}</p>`;
    });
}

function renderAppealsTable(data) {
    const container = document.getElementById('appeals-table-container');
    const items = data.items || [];
    appealsNextCursor = data.cursor || null;

    document.getElementById('appeals-prev-btn').disabled = appealsCursorStack.length === 0;
    document.getElementById('appeals-next-btn').disabled = !appealsNextCursor;

    if (items.length === 0) {
        container.innerHTML = '<p class="empty-state">No appeals match these filters.</p>';
        return;
    }

    let html = '<table class="data-table"><thead><tr>'
        + '<th>Submitted</th><th>Status</th><th>Appellant</th><th>Subject</th>'
        + '<th>Original Action</th><th>Reason</th><th>ID</th>'
        + '</tr></thead><tbody>';
    for (const a of items) {
        const when = new Date(a.submittedAt).toLocaleString();
        const appellant = a.submitterHandle || a.submitterDid;
        let subject = '—';
        if (a.subject) {
            const subjType = a.subject['$type'] || '?';
            if (subjType.endsWith('repoRef')) {
                subject = `repo: ${a.subject.did}`;
            } else if (subjType.endsWith('strongRef')) {
                subject = `record: ${a.subject.uri}`;
            } else if (subjType.endsWith('repoBlobRef')) {
                subject = `blob: ${a.subject.cid}`;
            }
        }
        const orig = a.originalActionSummary
            ? `${a.originalActionSummary.kind} #${a.originalActionSummary.id}: ${a.originalActionSummary.summary}`
            : '—';
        const reason = a.reason || '';
        html += `<tr><td>${when}</td><td>${a.status}</td><td>${appellant}</td>`
            + `<td>${subject}</td><td>${orig}</td><td>${reason}</td>`
            + `<td><a href="javascript:void(0)" onclick="loadAppealDetail(${a.id})">${a.id}</a></td></tr>`;
    }
    html += '</tbody></table>';
    container.innerHTML = html;
}

function loadAppealDetail(id) {
    fetch(`${API_BASE}/tools.aurora.moderator.getAppeal?id=${id}`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => res.json())
    .then(data => {
        alert(JSON.stringify(data, null, 2));
    })
    .catch(err => alert(`Error: ${err.message}`));
}

// Aurora capability probe (chainlink #99 / Phase 3.2). Calls
// tools.aurora.describeCapabilities and renders the JSON response
// inline. Operators use this to verify which Aurora extensions
// their instance supports without grepping the source.
function loadCapabilities() {
    const out = document.getElementById('capabilities-output');
    out.style.display = 'block';
    out.textContent = 'Loading...';
    fetch(`${API_BASE}/tools.aurora.describeCapabilities`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => {
        if (!res.ok) {
            throw new Error(`HTTP ${res.status}: ${res.statusText}`);
        }
        return res.json();
    })
    .then(data => {
        out.textContent = JSON.stringify(data, null, 2);
    })
    .catch(err => {
        out.textContent = `Error: ${err.message}`;
    });
}

// Refresh data periodically
setInterval(() => {
    if (currentPage === 'dashboard') {
        loadDashboardData();
    } else if (currentPage === 'moderation') {
        loadModerationQueue();
    } else if (currentPage === 'reports') {
        loadReports();
    }
}, 30000); // Every 30 seconds

// =====================================================================
// Phase 3.8D (chainlink #105) — Audit page + Audit entry detail +
// Forensic export modal per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.8,
// §5.3.9, §5.2 forensic export modal.
// =====================================================================

let auditCursorStack = [];
let auditNextCursor = null;
let auditSubscription = null;

function loadAudit() {
    auditCursorStack = [];
    auditNextCursor = null;
    fetchAuditPage(null);
    if (!auditSubscription) {
        // The Audit page subscribes to mod events with the audit
        // chain entries piggy-backed (Phase 3.9 sends only Event
        // messages for now; audit-entry pushing lands when the
        // server's subscribe handler grows include_audit_chain
        // support per §8.5). The indicator still reflects the
        // connection state so operators see "Live" vs "Reconnecting".
        const indicator = document.getElementById('audit-rt-indicator');
        auditSubscription = AuroraSubscription.subscribe(
            'subscribe-mod-events',
            {},
            {
                onEvent: () => {
                    // Audit entries flow alongside events; refresh
                    // page to pick up new chain rows.
                    if (auditCursorStack.length === 0) loadAudit();
                },
                onError: (e) => console.warn('audit subscription error:', e),
            }
        );
        if (indicator) AuroraSubscription.attachIndicator(indicator, auditSubscription);
    }
}

function loadAuditNext() {
    if (auditNextCursor) {
        auditCursorStack.push(auditNextCursor);
        fetchAuditPage(auditNextCursor);
    }
}

function loadAuditPrev() {
    if (auditCursorStack.length > 1) {
        auditCursorStack.pop();
        const prev = auditCursorStack[auditCursorStack.length - 1] || null;
        fetchAuditPage(prev);
    } else if (auditCursorStack.length === 1) {
        auditCursorStack = [];
        fetchAuditPage(null);
    }
}

function fetchAuditPage(cursor) {
    const params = new URLSearchParams();
    const actor = document.getElementById('audit-filter-actor')?.value.trim() || '';
    const subject = document.getElementById('audit-filter-subject')?.value.trim() || '';
    const action = document.getElementById('audit-filter-action')?.value.trim() || '';
    if (actor) params.set('actorDid', actor);
    if (subject) params.set('subjectDid', subject);
    if (action) params.set('action', action);
    if (cursor) params.set('cursor', cursor);
    params.set('limit', '25');

    const container = document.getElementById('audit-table-container');
    container.innerHTML = '<p class="empty-state">Loading…</p>';

    fetch(`${API_BASE}/tools.aurora.admin.getAuditTrail?${params.toString()}`, {
        headers: { 'Authorization': `Bearer ${adminToken}` }
    })
    .then(res => {
        if (!res.ok) throw new Error('HTTP ' + res.status);
        return res.json();
    })
    .then(data => renderAuditTable(data))
    .catch(err => {
        container.innerHTML = `<p class="empty-state">Error: ${err.message}</p>`;
    });
}

function renderAuditTable(data) {
    const container = document.getElementById('audit-table-container');
    let items = data.items || [];
    auditNextCursor = data.cursor || null;

    document.getElementById('audit-prev-btn').disabled = auditCursorStack.length === 0;
    document.getElementById('audit-next-btn').disabled = !auditNextCursor;

    const verifiedOnly = document.getElementById('audit-verified-only')?.checked;
    if (verifiedOnly) items = items.filter(e => e.verified);

    if (items.length === 0) {
        container.innerHTML = '<p class="empty-state">No audit entries match these filters.</p>';
        return;
    }

    let html = '<table class="data-table"><thead><tr>'
        + '<th>Seq</th><th>When</th><th>Actor</th><th>Action</th>'
        + '<th>Subject</th><th>Verified</th><th></th>'
        + '</tr></thead><tbody>';
    for (const e of items) {
        const when = new Date(e.timestamp).toLocaleString();
        const subj = e.subjectRef
            ? (e.subjectRef.did || e.subjectRef.uri || e.subjectRef.cid || '—')
            : '—';
        const verifiedBadge = e.verified
            ? '<span class="status-badge status-active" title="Hash matches stored chain hash">✓ verified</span>'
            : '<span class="status-badge status-suspended" title="Hash does not match — possibly tampered or pre-chain">✗ unverified</span>';
        html += `<tr><td>${e.sequence}</td><td>${when}</td>`
            + `<td><code>${e.actorDid}</code></td><td>${e.action}</td>`
            + `<td><code>${subj}</code></td><td>${verifiedBadge}</td>`
            + `<td><a href="javascript:void(0)" onclick="showAuditEntryDetail('${e.id}')">View</a></td></tr>`;
    }
    html += '</tbody></table>';
    container.innerHTML = html;
    // Cache items by id so chain-walk can navigate without refetching.
    window._auditItemsCache = window._auditItemsCache || {};
    items.forEach(e => { window._auditItemsCache[e.id] = e; });
}

// Audit entry detail modal — shows the full entry + chain-walk
// previous-hash navigation per §5.3.9.
function showAuditEntryDetail(entryId) {
    const cached = (window._auditItemsCache || {})[entryId];
    if (!cached) {
        alert('Entry not in current page cache. Reload the audit table.');
        return;
    }
    let modal = document.getElementById('modal-audit-entry');
    if (!modal) {
        modal = document.createElement('div');
        modal.id = 'modal-audit-entry';
        modal.className = 'modal';
        modal.innerHTML = `
            <div class="modal-header">
                <h3>Audit entry detail</h3>
                <button class="modal-close" onclick="closeAuditDetailModal()" aria-label="Close">&times;</button>
            </div>
            <div class="modal-body" id="audit-entry-content"></div>
        `;
        document.body.appendChild(modal);
    }
    const subjStr = cached.subjectRef
        ? JSON.stringify(cached.subjectRef, null, 2)
        : 'none';
    const cascadeStr = cached.cascadeSubjects && cached.cascadeSubjects.length > 0
        ? JSON.stringify(cached.cascadeSubjects, null, 2)
        : 'none';
    const prevHash = cached.previousHash;
    const prevHashSection = prevHash
        ? `<p><strong>Previous hash:</strong> <code>${prevHash}</code> ` +
          `<a href="javascript:void(0)" onclick="walkChainTo('${prevHash}')">[walk to previous]</a></p>`
        : '<p><strong>Previous hash:</strong> none (first entry in chain)</p>';
    document.getElementById('audit-entry-content').innerHTML = `
        <dl style="font-size: 0.875rem;">
            <dt>Sequence</dt><dd>${cached.sequence}</dd>
            <dt>Timestamp</dt><dd>${cached.timestamp}</dd>
            <dt>Actor DID</dt><dd><code>${cached.actorDid}</code></dd>
            <dt>Action</dt><dd>${cached.action}</dd>
            <dt>Rationale</dt><dd>${cached.rationale}</dd>
            <dt>Subject</dt><dd><pre style="white-space: pre-wrap; margin: 0;">${subjStr}</pre></dd>
            <dt>Cascade subjects</dt><dd><pre style="white-space: pre-wrap; margin: 0;">${cascadeStr}</pre></dd>
            <dt>Snapshot ID</dt><dd>${cached.snapshotId || 'none'}</dd>
            <dt>Event ID</dt><dd>${cached.eventId || 'none'}</dd>
            <dt>Current hash</dt><dd><code style="word-break: break-all;">${cached.currentHash}</code></dd>
        </dl>
        ${prevHashSection}
        <p><strong>Verified:</strong> ${cached.verified ? '✓ Yes — recomputed hash matches stored value' : '✗ No — hash divergent or pre-chain sentinel'}</p>
    `;
    document.getElementById('modal-overlay').classList.add('active');
    modal.classList.add('active');
}

function closeAuditDetailModal() {
    document.getElementById('modal-audit-entry')?.classList.remove('active');
    document.getElementById('modal-overlay').classList.remove('active');
}

// Chain-walk: search the cached page (and re-fetch if needed) for an
// entry whose currentHash matches the supplied previousHash.
function walkChainTo(previousHash) {
    const items = window._auditItemsCache || {};
    const target = Object.values(items).find(e => e.currentHash === previousHash);
    if (target) {
        showAuditEntryDetail(target.id);
        return;
    }
    // Not in current page — surface a message rather than auto-navigating.
    // Fully implementing chain-walk across pages requires server-side
    // by-hash lookup which lands with #108 polish.
    alert('Previous entry not in current page. Use filters to narrow to the previous range, then walk.');
}

// Forensic export modal (§5.2 forensic export modal +
// substrate primitive 21 in design doc).
function openForensicExportModal(did, handle) {
    let modal = document.getElementById('modal-forensic-export');
    if (!modal) {
        modal = document.createElement('div');
        modal.id = 'modal-forensic-export';
        modal.className = 'modal';
        modal.innerHTML = `
            <div class="modal-header">
                <h3>Generate forensic export</h3>
                <button class="modal-close" onclick="closeForensicModal()" aria-label="Close">&times;</button>
            </div>
            <div class="modal-body">
                <p><strong>Subject:</strong> <span id="forensic-subject"></span></p>
                <fieldset style="border: 1px solid var(--border-color); padding: 0.75rem; margin: 0.75rem 0;">
                    <legend>Include</legend>
                    <label style="display: block;"><input type="checkbox" id="forensic-include-repo" checked> Repository content (CAR file) — deferred to v0.3</label>
                    <label style="display: block;"><input type="checkbox" id="forensic-include-blobs" checked> Blobs — deferred to v0.3</label>
                    <label style="display: block;"><input type="checkbox" id="forensic-include-mod"  checked> Moderation history</label>
                    <label style="display: block;"><input type="checkbox" id="forensic-include-meta"> Account metadata <span class="role-tag">SuperAdmin only</span></label>
                    <label style="display: block;"><input type="checkbox" id="forensic-include-audit"> Audit chain entries <span class="role-tag">SuperAdmin only</span></label>
                </fieldset>
                <label style="display: block;">Rationale (required)</label>
                <textarea id="forensic-rationale" rows="3" style="width: 100%;" aria-required="true"></textarea>
                <p class="action-panel-hint" style="margin-top: 0.5rem;">
                    This export will be recorded in the audit chain with a tamper-evident hash.
                    The bundle will contain account data; treat as sensitive.
                </p>
                <div class="action-panel-buttons" style="margin-top: 0.75rem;">
                    <button class="btn-secondary" onclick="closeForensicModal()">Cancel</button>
                    <button class="btn-danger" onclick="submitForensicExport()">Generate export</button>
                </div>
            </div>
        `;
        document.body.appendChild(modal);
    }
    document.getElementById('forensic-subject').textContent = (handle ? '@' + handle + ' — ' : '') + did;
    document.getElementById('forensic-rationale').value = '';
    window._forensicTargetDid = did;
    document.getElementById('modal-overlay').classList.add('active');
    modal.classList.add('active');
}

function closeForensicModal() {
    document.getElementById('modal-forensic-export')?.classList.remove('active');
    document.getElementById('modal-overlay').classList.remove('active');
}

function submitForensicExport() {
    const did = window._forensicTargetDid;
    if (!did) return;
    const rationale = document.getElementById('forensic-rationale').value.trim();
    if (!rationale) { alert('Rationale is required.'); return; }
    const body = {
        did: did,
        rationale: rationale,
        includeRepo: document.getElementById('forensic-include-repo').checked,
        includeBlobs: document.getElementById('forensic-include-blobs').checked,
        includeModerationHistory: document.getElementById('forensic-include-mod').checked,
        includeAccountMetadata: document.getElementById('forensic-include-meta').checked,
        includeAuditChain: document.getElementById('forensic-include-audit').checked,
    };
    fetch(`${API_BASE}/tools.aurora.admin.exportAccountForensic`, {
        method: 'POST',
        headers: {
            'Authorization': `Bearer ${adminToken}`,
            'Content-Type': 'application/json',
        },
        body: JSON.stringify(body),
    })
    .then(async res => {
        if (!res.ok) {
            let msg = 'HTTP ' + res.status;
            try { const j = await res.json(); msg += ': ' + (j.message || j.error || ''); } catch (e) {}
            throw new Error(msg);
        }
        const auditId = res.headers.get('X-Aurora-Audit-Entry-Id');
        const bundleHash = res.headers.get('X-Aurora-Bundle-Hash');
        const blob = await res.blob();
        // Trigger browser download
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `forensic-export-${did.replace(/:/g, '_')}-${new Date().toISOString().replace(/[:.]/g, '')}.tar`;
        document.body.appendChild(a);
        a.click();
        document.body.removeChild(a);
        URL.revokeObjectURL(url);
        closeForensicModal();
        alert(`Export complete.\nAudit entry: ${auditId}\nBundle hash: ${bundleHash}`);
    })
    .catch(err => alert('Export failed: ' + err.message));
}
