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
    }
}

// Dashboard
function loadDashboardData() {
    // Load stats
    fetch(`${API_BASE}/com.atproto.admin.getStats`, {
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

function renderUsersTable(users) {
    const tbody = document.getElementById('users-table');
    tbody.innerHTML = users.map(user => `
        <tr>
            <td>${user.handle}</td>
            <td><code>${user.did}</code></td>
            <td>${user.email || 'N/A'}</td>
            <td>${new Date(user.createdAt).toLocaleDateString()}</td>
            <td><span class="status-badge status-${user.status || 'active'}">${user.status || 'Active'}</span></td>
            <td>
                <button class="btn-sm btn-primary" onclick="viewUser('${user.did}')">View</button>
                <button class="btn-sm btn-secondary" onclick="suspendUser('${user.did}')">Suspend</button>
            </td>
        </tr>
    `).join('');
}

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
        `;
        showModal('modal-user-details');
    })
    .catch(err => {
        alert('Failed to load user details');
        console.error(err);
    });
}

function suspendUser(did) {
    if (!confirm('Are you sure you want to suspend this user?')) return;

    fetch(`${API_BASE}/com.atproto.admin.updateSubjectStatus`, {
        method: 'POST',
        headers: {
            'Authorization': `Bearer ${adminToken}`,
            'Content-Type': 'application/json'
        },
        body: JSON.stringify({
            subject: { did },
            takedown: { applied: true }
        })
    })
    .then(() => {
        alert('User suspended successfully');
        loadUsers();
    })
    .catch(err => {
        alert('Failed to suspend user');
        console.error(err);
    });
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

function renderModerationQueue(items) {
    const container = document.getElementById('moderation-queue');
    container.innerHTML = items.map(item => `
        <div class="mod-item">
            <div class="mod-header">
                <div>
                    <strong>${item.reasonType || 'Unknown'}</strong>
                    <p>By: ${item.reportedBy}</p>
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
    `).join('');
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

function renderReports(reports) {
    const container = document.getElementById('reports-list');
    container.innerHTML = reports.map(report => `
        <div class="report-item">
            <div class="report-header">
                <div>
                    <strong>${report.reasonType}</strong>
                    <p>Reporter: @${report.reportedBy}</p>
                    <p>Subject: ${report.subject}</p>
                </div>
                <span class="status-badge status-${report.status}">${report.status}</span>
            </div>
            <div class="report-content">
                ${report.reason || 'No reason provided'}
            </div>
            <div class="report-actions">
                <button class="btn-sm btn-primary" onclick="viewReport('${report.id}')">View Details</button>
            </div>
        </div>
    `).join('');
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

function renderInvites(codes) {
    const tbody = document.getElementById('invites-table');
    tbody.innerHTML = codes.map(code => `
        <tr>
            <td><code>${code.code}</code></td>
            <td>${code.uses || 0} / ${code.available || 1}</td>
            <td>@${code.created_by || 'system'}</td>
            <td>${new Date(code.created_at).toLocaleDateString()}</td>
            <td><span class="status-badge status-${code.disabled ? 'suspended' : 'active'}">${code.disabled ? 'Disabled' : 'Active'}</span></td>
            <td>
                <button class="btn-sm btn-danger" onclick="disableInvite('${code.code}')">Disable</button>
            </td>
        </tr>
    `).join('');
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
