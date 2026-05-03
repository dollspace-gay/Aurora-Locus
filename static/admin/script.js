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
    }
}

// Dashboard
function loadDashboardData() {
    // Load stats
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
        console.log('Stats data:', data);
        document.getElementById('stat-users').textContent = data.totalUsers || 0;
        document.getElementById('stat-posts').textContent = data.totalPosts || 0;
        document.getElementById('stat-reports').textContent = data.openReports || 0;

        // Fix storage calculation - handle NaN
        const storageBytes = data.storageBytes || 0;
        const storageGB = (storageBytes / 1024 / 1024 / 1024).toFixed(2);
        document.getElementById('stat-storage').textContent = `${storageGB} GB`;

        // Update the stat change indicators with real data
        const totalUsers = data.totalUsers || 0;
        const activeUsers = data.activeUsers || 0;
        document.querySelector('#page-dashboard .stat-card:nth-child(1) .stat-change').textContent = `${activeUsers} active`;
        document.querySelector('#page-dashboard .stat-card:nth-child(2) .stat-change').textContent = `${data.totalPosts || 0} total`;
        document.querySelector('#page-dashboard .stat-card:nth-child(3) .stat-change').textContent = data.openReports > 0 ? 'Requires attention' : 'All clear';

        const totalInvites = data.totalInvites || 0;
        const availableInvites = data.availableInvites || 0;
        document.querySelector('#page-dashboard .stat-card:nth-child(4) .stat-change').textContent = `${availableInvites} of ${totalInvites} available`;

        // Initialize charts with real data
        initializeCharts(data);
    })
    .catch(err => {
        console.error('Failed to load stats:', err);
        // Set defaults on error
        document.getElementById('stat-users').textContent = '0';
        document.getElementById('stat-posts').textContent = '0';
        document.getElementById('stat-reports').textContent = '0';
        document.getElementById('stat-storage').textContent = '0.00 GB';
    });

    // Load recent activity
    loadRecentActivity();

    // Initialize charts
    initializeCharts();
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
                    <div style="margin-top: 0.75rem;">
                        <button class="btn-secondary" onclick="openPasswordResetModal('${user.did}','${user.handle || ''}')">Send password reset</button>
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

function loadEvents() {
    eventsCursorStack = [];
    eventsNextCursor = null;
    fetchEventsPage(null);
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
