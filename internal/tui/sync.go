package tui

import (
	"bufio"
	"bytes"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/gnur/exokephalos/internal/assets"
	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/syncsvc"
	"github.com/gnur/exokephalos/internal/version"
)

func startSyncCmd(baseDir string, c *cache.Cache, appCfg *config.AppConfig) tea.Cmd {
	return func() tea.Msg {
		if c == nil || appCfg == nil || appCfg.Sync.ServerURL == "" {
			return syncMsg{status: "error", err: fmt.Errorf("sync is not configured")}
		}
		if err := checkServerVersion(appCfg.Sync.ServerURL); err != nil {
			return syncMsg{status: "version mismatch", err: err}
		}
		pub, priv, err := syncsvc.EnsureKeypair(appCfg.Sync.KeyPath)
		if err != nil {
			return syncMsg{status: "error", err: err}
		}
		clientID := appCfg.Sync.ClientID
		if clientID == "" {
			host, _ := os.Hostname()
			clientID = host
		}
		status, err := ensureApproved(appCfg.Sync.ServerURL, clientID, pub, c)
		if err != nil {
			return syncMsg{status: "offline", err: err, retryStart: true}
		}
		if status != "approved" {
			return syncMsg{status: "pending approval", retryStart: true}
		}
		changes, err := syncsvc.BuildLocalChanges(baseDir, c, true)
		if err != nil {
			return syncMsg{status: "error", err: err}
		}
		if err := syncsvc.EnqueueChanges(c, changes); err != nil {
			return syncMsg{status: "error", err: err}
		}
		if err := c.SetSyncStarted(true); err != nil {
			return syncMsg{status: "error", err: err}
		}
		if err := pushOutbox(appCfg.Sync.ServerURL, clientID, priv, c); err != nil {
			return syncMsg{status: "offline", err: err, retrySync: true}
		}
		cfgChanged, err := pullSnapshot(baseDir, appCfg.Sync.ServerURL, clientID, priv, c)
		if err != nil {
			return syncMsg{status: "offline", err: err, retrySync: true}
		}
		return syncMsg{status: "connected", startListen: true, retrySync: true, configChanged: cfgChanged}
	}
}

func syncStartupCmd(baseDir string, c *cache.Cache, appCfg *config.AppConfig) tea.Cmd {
	return func() tea.Msg {
		if c == nil || appCfg == nil || appCfg.Sync.ServerURL == "" || !c.IsSyncStarted() {
			return nil
		}
		if err := checkServerVersion(appCfg.Sync.ServerURL); err != nil {
			return syncMsg{status: "version mismatch", err: err}
		}
		_, priv, err := syncsvc.EnsureKeypair(appCfg.Sync.KeyPath)
		if err != nil {
			return syncMsg{status: "error", err: err, retrySync: true}
		}
		if err := enqueueRootConfigs(baseDir, c); err != nil {
			return syncMsg{status: "error", err: err, retrySync: true}
		}
		clientID := appCfg.Sync.ClientID
		if clientID == "" {
			host, _ := os.Hostname()
			clientID = host
		}
		if err := pushOutbox(appCfg.Sync.ServerURL, clientID, priv, c); err != nil {
			return syncMsg{status: "offline", err: err, retrySync: true}
		}
		cfgChanged, err := pullSnapshot(baseDir, appCfg.Sync.ServerURL, clientID, priv, c)
		if err != nil {
			return syncMsg{status: "error", err: err, retrySync: true}
		}
		return syncMsg{status: "connected", startListen: true, retrySync: true, configChanged: cfgChanged}
	}
}

func reconcileSyncCmd(baseDir string, c *cache.Cache, appCfg *config.AppConfig) tea.Cmd {
	return func() tea.Msg {
		if c == nil || appCfg == nil || appCfg.Sync.ServerURL == "" || !c.IsSyncStarted() {
			return nil
		}
		if err := checkServerVersion(appCfg.Sync.ServerURL); err != nil {
			return syncMsg{status: "version mismatch", err: err}
		}
		_, priv, err := syncsvc.EnsureKeypair(appCfg.Sync.KeyPath)
		if err != nil {
			return syncMsg{status: "error", err: err, retrySync: true}
		}
		if err := enqueueRootConfigs(baseDir, c); err != nil {
			return syncMsg{status: "error", err: err, retrySync: true}
		}
		if err := c.Sync(); err != nil {
			return syncMsg{status: "error", err: err, retrySync: true}
		}
		clientID := appCfg.Sync.ClientID
		if clientID == "" {
			host, _ := os.Hostname()
			clientID = host
		}
		if err := pushOutbox(appCfg.Sync.ServerURL, clientID, priv, c); err != nil {
			return syncMsg{status: "offline", err: err, retrySync: true}
		}
		cfgChanged, err := pullSnapshot(baseDir, appCfg.Sync.ServerURL, clientID, priv, c)
		if err != nil {
			return syncMsg{status: "offline", err: err, retrySync: true}
		}
		return syncMsg{status: "connected", retrySync: true, configChanged: cfgChanged}
	}
}

func checkServerVersion(serverURL string) error {
	req, err := http.NewRequest(http.MethodGet, strings.TrimRight(serverURL, "/")+"/api/sync/version", nil)
	if err != nil {
		return err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("checking sync server version: %s", resp.Status)
	}
	var payload struct {
		Version string `json:"version"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		return fmt.Errorf("decoding sync server version: %w", err)
	}
	if payload.Version == "" {
		return fmt.Errorf("sync server did not report a version")
	}
	if payload.Version != version.Version {
		return fmt.Errorf("sync server version %s does not match local version %s", payload.Version, version.Version)
	}
	return nil
}

func syncTickCmd(after time.Duration) tea.Cmd {
	return tea.Tick(after, func(time.Time) tea.Msg {
		return syncTickMsg{}
	})
}

func syncStartTickCmd(after time.Duration) tea.Cmd {
	return tea.Tick(after, func(time.Time) tea.Msg {
		return syncStartTickMsg{}
	})
}

func pushOutboxCmd(c *cache.Cache, appCfg *config.AppConfig) tea.Cmd {
	return func() tea.Msg {
		if c == nil || appCfg == nil || appCfg.Sync.ServerURL == "" {
			return nil
		}
		_, priv, err := syncsvc.EnsureKeypair(appCfg.Sync.KeyPath)
		if err != nil {
			return syncMsg{status: "error", err: err}
		}
		clientID := appCfg.Sync.ClientID
		if clientID == "" {
			host, _ := os.Hostname()
			clientID = host
		}
		if err := pushOutbox(appCfg.Sync.ServerURL, clientID, priv, c); err != nil {
			return syncMsg{status: "offline", err: err}
		}
		return syncMsg{status: "connected", startListen: true, retrySync: true}
	}
}

func syncListenCmd(baseDir string, c *cache.Cache, appCfg *config.AppConfig) tea.Cmd {
	return func() tea.Msg {
		if c == nil || appCfg == nil || appCfg.Sync.ServerURL == "" || !c.IsSyncStarted() {
			return nil
		}
		_, priv, err := syncsvc.EnsureKeypair(appCfg.Sync.KeyPath)
		if err != nil {
			return syncMsg{status: "error", err: err, retryListen: true}
		}
		clientID := appCfg.Sync.ClientID
		if clientID == "" {
			host, _ := os.Hostname()
			clientID = host
		}
		revision, err := listenForServerEvent(appCfg.Sync.ServerURL, clientID, priv, c)
		if err != nil {
			return syncMsg{status: "offline", err: err, retryListen: true}
		}
		if revision > 0 {
			_ = c.SetMeta("sync_last_revision", fmt.Sprintf("%d", revision))
		}
		cfgChanged, err := pullSnapshot(baseDir, appCfg.Sync.ServerURL, clientID, priv, c)
		if err != nil {
			return syncMsg{status: "offline", err: err, retryListen: true, retrySync: true}
		}
		return syncMsg{status: "connected", startListen: true, retrySync: true, configChanged: cfgChanged}
	}
}

func ensureApproved(serverURL, clientID string, pub ed25519.PublicKey, c *cache.Cache) (string, error) {
	token, _ := c.Meta("sync_enrollment_token")
	if token != "" {
		req, _ := http.NewRequest(http.MethodGet, strings.TrimRight(serverURL, "/")+"/api/sync/enroll/status?client_id="+clientID+"&token="+token, nil)
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			return "", err
		}
		defer resp.Body.Close()
		if resp.StatusCode == http.StatusOK {
			var out map[string]string
			_ = json.NewDecoder(resp.Body).Decode(&out)
			return out["status"], nil
		}
	}
	body, _ := json.Marshal(map[string]string{
		"client_id":  clientID,
		"label":      clientID,
		"public_key": base64.StdEncoding.EncodeToString(pub),
	})
	resp, err := http.Post(strings.TrimRight(serverURL, "/")+"/api/sync/enroll", "application/json", bytes.NewReader(body))
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return "", fmt.Errorf("enroll failed: %s", resp.Status)
	}
	var out map[string]string
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return "", err
	}
	if out["enrollment_token"] != "" {
		_ = c.SetMeta("sync_enrollment_token", out["enrollment_token"])
	}
	return out["status"], nil
}

func pushOutbox(serverURL, clientID string, priv ed25519.PrivateKey, c *cache.Cache) error {
	entries, err := c.PendingOutbox(100)
	if err != nil {
		return err
	}
	if len(entries) == 0 {
		return nil
	}
	var changes []syncsvc.Change
	var ids []int64
	for _, entry := range entries {
		var ch syncsvc.Change
		if err := json.Unmarshal([]byte(entry.Payload), &ch); err != nil {
			_ = c.MarkOutboxFailed(entry.ID, err.Error())
			continue
		}
		if ch.TargetKind == "asset" {
			if ch.Op != "delete_asset" && ch.Op != "delete" {
				if err := uploadSyncAsset(serverURL, clientID, priv, c.BaseDir(), ch.Path); err != nil {
					_ = c.MarkOutboxFailed(entry.ID, err.Error())
					continue
				}
			}
			if err := pushChanges(serverURL, clientID, priv, []syncsvc.Change{ch}); err != nil {
				_ = c.MarkOutboxFailed(entry.ID, err.Error())
				continue
			}
			_ = c.MarkOutboxSynced(entry.ID)
			continue
		}
		changes = append(changes, ch)
		ids = append(ids, entry.ID)
	}
	if len(changes) == 0 {
		return nil
	}
	if err := pushChanges(serverURL, clientID, priv, changes); err != nil {
		for _, id := range ids {
			_ = c.MarkOutboxFailed(id, err.Error())
		}
		return err
	}
	for _, id := range ids {
		_ = c.MarkOutboxSynced(id)
	}
	return nil
}

func pushChanges(serverURL, clientID string, priv ed25519.PrivateKey, changes []syncsvc.Change) error {
	body, _ := json.Marshal(map[string]interface{}{"changes": changes})
	url := strings.TrimRight(serverURL, "/") + "/api/sync/changes"
	req, err := http.NewRequest(http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	syncsvc.SignRequest(req, body, clientID, priv)
	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return fmt.Errorf("push failed: %s", resp.Status)
	}
	return nil
}

func uploadSyncAsset(serverURL, clientID string, priv ed25519.PrivateKey, baseDir, rel string) error {
	path, err := assets.Path(baseDir, rel)
	if err != nil {
		return err
	}
	url := strings.TrimRight(serverURL, "/") + "/api/sync/assets/" + strings.TrimPrefix(filepath.ToSlash(rel), "assets/")
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	req, err := http.NewRequest(http.MethodPut, url, bytes.NewReader(data))
	if err != nil {
		return err
	}
	syncsvc.SignRequest(req, data, clientID, priv)
	resp, err := (&http.Client{Timeout: 30 * time.Second}).Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return fmt.Errorf("asset upload failed: %s", resp.Status)
	}
	return nil
}

func pullSnapshot(baseDir, serverURL, clientID string, priv ed25519.PrivateKey, c *cache.Cache) (bool, error) {
	url := strings.TrimRight(serverURL, "/") + "/api/sync/snapshot"
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return false, err
	}
	SignBody := []byte(nil)
	syncsvc.SignRequest(req, SignBody, clientID, priv)
	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return false, err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return false, fmt.Errorf("snapshot failed: %s", resp.Status)
	}
	var snapshot struct {
		Items   []syncsvc.Change `json:"items"`
		Configs []syncsvc.Change `json:"configs"`
		Assets  []syncsvc.Change `json:"assets"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&snapshot); err != nil {
		return false, err
	}
	snapshotPaths := make(map[string]bool)
	configChanged := false

	for _, ch := range snapshot.Configs {
		if ch.Path == "" {
			continue
		}
		path := filepath.Clean(filepath.Join(baseDir, ch.Path))
		if !strings.HasPrefix(path, filepath.Clean(baseDir)+string(filepath.Separator)) {
			continue
		}
		snapshotPaths[ch.Path] = true
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			return false, err
		}

		// Read existing file to check for changes
		existing, err := os.ReadFile(path)
		if err != nil || !bytes.Equal(existing, []byte(ch.Content)) {
			configChanged = true
		}

		if err := os.WriteFile(path, []byte(ch.Content), 0644); err != nil {
			return false, err
		}
		_ = c.SetMeta(configHashMetaKey(ch.Path), contentHashString(ch.Content))
	}
	for _, ch := range snapshot.Items {
		if ch.Path == "" || ch.ID == "" {
			continue
		}
		path := filepath.Clean(filepath.Join(baseDir, ch.Path))
		if !strings.HasPrefix(path, filepath.Clean(baseDir)+string(filepath.Separator)) {
			continue
		}
		snapshotPaths[ch.Path] = true
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			return false, err
		}
		if err := markdown.WriteFrontmatter(path, ch.Frontmatter, ch.Body); err != nil {
			return false, err
		}
		_ = c.NotifyWriteNoOutbox(path)
	}
	for _, ch := range snapshot.Assets {
		if ch.Path == "" {
			continue
		}
		if _, err := assets.Path(baseDir, ch.Path); err != nil {
			continue
		}
		if ch.Deleted || ch.Op == "delete_asset" {
			path, _ := assets.Path(baseDir, ch.Path)
			_ = os.Remove(path)
			continue
		}
		local, err := assets.Inspect(baseDir, ch.Path)
		if err == nil && local.Hash == ch.Hash {
			continue
		}
		if err := downloadSyncAsset(serverURL, clientID, priv, baseDir, ch.Path); err != nil {
			return false, err
		}
	}

	// Retrieve all paths currently in the local outbox to avoid deleting local unsynced files.
	outboxPaths := make(map[string]bool)
	if c.DB() != nil {
		if outboxRows, err := c.DB().Query(`SELECT path FROM outbox WHERE status IN ('pending', 'failed')`); err == nil {
			defer outboxRows.Close()
			for outboxRows.Next() {
				var p string
				if err := outboxRows.Scan(&p); err == nil {
					outboxPaths[p] = true
				}
			}
		}
	}

	// Delete local items that are not in the snapshot and have no pending changes.
	if c.DB() != nil {
		if localRows, err := c.DB().Query(`SELECT path FROM items WHERE deleted_at = ''`); err == nil {
			defer localRows.Close()
			var localPaths []string
			for localRows.Next() {
				var p string
				if err := localRows.Scan(&p); err == nil {
					localPaths = append(localPaths, p)
				}
			}
			for _, localPath := range localPaths {
				if !snapshotPaths[localPath] && !outboxPaths[localPath] {
					absPath := filepath.Clean(filepath.Join(baseDir, localPath))
					if strings.HasPrefix(absPath, filepath.Clean(baseDir)+string(filepath.Separator)) {
						_ = os.Remove(absPath)
						_ = c.NotifyDeleteNoOutbox(absPath)
					}
				}
			}
		}
	}

	// Delete synced workspace config files that are absent from the snapshot.
	for _, cfgPath := range workspaceConfigFiles(baseDir) {
		rel, err := filepath.Rel(baseDir, cfgPath)
		if err != nil {
			continue
		}
		if !snapshotPaths[rel] && !outboxPaths[rel] {
			absPath := filepath.Clean(cfgPath)
			if strings.HasPrefix(absPath, filepath.Clean(baseDir)+string(filepath.Separator)) {
				_ = os.Remove(absPath)
				_ = c.SetMeta(configHashMetaKey(rel), "")
				configChanged = true
			}
		}
	}

	return configChanged, nil
}

func downloadSyncAsset(serverURL, clientID string, priv ed25519.PrivateKey, baseDir, rel string) error {
	url := strings.TrimRight(serverURL, "/") + "/api/sync/assets/" + strings.TrimPrefix(filepath.ToSlash(rel), "assets/")
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	syncsvc.SignRequest(req, nil, clientID, priv)
	resp, err := (&http.Client{Timeout: 30 * time.Second}).Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return fmt.Errorf("asset download failed: %s", resp.Status)
	}
	_, err = assets.Store(baseDir, rel, resp.Body)
	return err
}

func listenForServerEvent(serverURL, clientID string, priv ed25519.PrivateKey, c *cache.Cache) (int64, error) {
	since, _ := c.Meta("sync_last_revision")
	url := strings.TrimRight(serverURL, "/") + "/api/sync/events"
	if since != "" {
		url += "?since_revision=" + since
	}
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return 0, err
	}
	syncsvc.SignRequest(req, nil, clientID, priv)
	client := &http.Client{Timeout: 0}
	resp, err := client.Do(req)
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return 0, fmt.Errorf("events failed: %s", resp.Status)
	}
	scanner := bufio.NewScanner(resp.Body)
	var eventName string
	for scanner.Scan() {
		line := scanner.Text()
		if strings.HasPrefix(line, "event:") {
			eventName = strings.TrimSpace(strings.TrimPrefix(line, "event:"))
			continue
		}
		if !strings.HasPrefix(line, "data:") {
			continue
		}
		if eventName != "" && eventName != "change" {
			eventName = ""
			continue
		}
		var event struct {
			Revision int64 `json:"revision"`
		}
		if err := json.Unmarshal([]byte(strings.TrimSpace(strings.TrimPrefix(line, "data:"))), &event); err != nil {
			return 0, err
		}
		if event.Revision > 0 {
			return event.Revision, nil
		}
		eventName = ""
	}
	if err := scanner.Err(); err != nil {
		return 0, err
	}
	return 0, fmt.Errorf("event stream closed")
}

func enqueueRootConfigs(baseDir string, c *cache.Cache) error {
	for _, path := range workspaceConfigFiles(baseDir) {
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		rel, _ := filepath.Rel(baseDir, path)
		hash := contentHashString(string(data))
		if previous, _ := c.Meta(configHashMetaKey(rel)); previous == hash {
			continue
		}
		payload, _ := json.Marshal(syncsvc.Change{
			Op:         "upsert_config",
			TargetKind: "config",
			Path:       rel,
			Content:    string(data),
		})
		if err := c.EnqueueOutbox("upsert_config", "config", rel, rel, string(payload)); err != nil {
			return err
		}
		_ = c.SetMeta(configHashMetaKey(rel), hash)
	}
	return nil
}

func workspaceConfigFiles(baseDir string) []string {
	var files []string
	_ = filepath.WalkDir(baseDir, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return nil
		}
		rel, err := filepath.Rel(baseDir, path)
		if err != nil {
			return nil
		}
		if config.IsWorkspacePath(rel) {
			files = append(files, path)
		}
		return nil
	})
	return files
}

func configHashMetaKey(path string) string {
	return "sync_config_hash:" + filepath.ToSlash(path)
}

func contentHashString(content string) string {
	sum := sha256.Sum256([]byte(content))
	return hex.EncodeToString(sum[:])
}
