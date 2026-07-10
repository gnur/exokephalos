package tui

import (
	"bufio"
	"bytes"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/syncsvc"
)

func startSyncCmd(baseDir string, c *cache.Cache, appCfg *config.AppConfig) tea.Cmd {
	return func() tea.Msg {
		if c == nil || appCfg == nil || appCfg.Sync.ServerURL == "" {
			return syncMsg{status: "error", err: fmt.Errorf("sync is not configured")}
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
			return syncMsg{status: "offline", err: err}
		}
		if status != "approved" {
			return syncMsg{status: "pending approval"}
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
			return syncMsg{status: "offline", err: err}
		}
		if err := pullSnapshot(baseDir, appCfg.Sync.ServerURL, clientID, priv, c); err != nil {
			return syncMsg{status: "offline", err: err}
		}
		return syncMsg{status: "connected", startListen: true}
	}
}

func syncStartupCmd(baseDir string, c *cache.Cache, appCfg *config.AppConfig) tea.Cmd {
	return func() tea.Msg {
		if c == nil || appCfg == nil || appCfg.Sync.ServerURL == "" || !c.IsSyncStarted() {
			return nil
		}
		_, priv, err := syncsvc.EnsureKeypair(appCfg.Sync.KeyPath)
		if err != nil {
			return syncMsg{status: "error", err: err}
		}
		if err := enqueueRootConfigs(baseDir, c); err != nil {
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
		if err := pullSnapshot(baseDir, appCfg.Sync.ServerURL, clientID, priv, c); err != nil {
			return syncMsg{status: "offline", err: err}
		}
		return syncMsg{status: "connected", startListen: true}
	}
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
		return syncMsg{status: "connected", startListen: true}
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
		if err := pullSnapshot(baseDir, appCfg.Sync.ServerURL, clientID, priv, c); err != nil {
			return syncMsg{status: "offline", err: err, retryListen: true}
		}
		return syncMsg{status: "connected", startListen: true}
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
		changes = append(changes, ch)
		ids = append(ids, entry.ID)
	}
	if len(changes) == 0 {
		return nil
	}
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
		for _, id := range ids {
			_ = c.MarkOutboxFailed(id, err.Error())
		}
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		err := fmt.Errorf("push failed: %s", resp.Status)
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

func pullSnapshot(baseDir, serverURL, clientID string, priv ed25519.PrivateKey, c *cache.Cache) error {
	url := strings.TrimRight(serverURL, "/") + "/api/sync/snapshot"
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	SignBody := []byte(nil)
	syncsvc.SignRequest(req, SignBody, clientID, priv)
	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return fmt.Errorf("snapshot failed: %s", resp.Status)
	}
	var snapshot struct {
		Items   []syncsvc.Change `json:"items"`
		Configs []syncsvc.Change `json:"configs"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&snapshot); err != nil {
		return err
	}
	for _, ch := range snapshot.Configs {
		if ch.Path == "" {
			continue
		}
		path := filepath.Clean(filepath.Join(baseDir, ch.Path))
		if !strings.HasPrefix(path, filepath.Clean(baseDir)+string(filepath.Separator)) {
			continue
		}
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			return err
		}
		if err := os.WriteFile(path, []byte(ch.Content), 0644); err != nil {
			return err
		}
	}
	for _, ch := range snapshot.Items {
		if ch.Path == "" || ch.ID == "" {
			continue
		}
		path := filepath.Clean(filepath.Join(baseDir, ch.Path))
		if !strings.HasPrefix(path, filepath.Clean(baseDir)+string(filepath.Separator)) {
			continue
		}
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			return err
		}
		if err := markdown.WriteFrontmatter(path, ch.Frontmatter, ch.Body); err != nil {
			return err
		}
		_ = c.NotifyWrite(path)
	}
	return nil
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
	matches, err := filepath.Glob(filepath.Join(baseDir, "*.toml"))
	if err != nil {
		return err
	}
	for _, path := range matches {
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		rel, _ := filepath.Rel(baseDir, path)
		payload, _ := json.Marshal(syncsvc.Change{
			Op:         "upsert_config",
			TargetKind: "config",
			Path:       rel,
			Content:    string(data),
		})
		if err := c.EnqueueOutbox("upsert_config", "config", rel, rel, string(payload)); err != nil {
			return err
		}
	}
	return nil
}
