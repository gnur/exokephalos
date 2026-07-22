package handlers

import (
	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/repo"
	"github.com/gnur/exokephalos/internal/scanner"
)

type ItemStore interface {
	All() ([]scanner.Item, error)
	GetByID(id string) (*scanner.Item, error)
	ReadRaw(path string) (string, error)
	WriteRaw(path, content string) error
	CreateItem(path string, fm map[string]interface{}, body string) error
	UpdateItem(path string, fm map[string]interface{}, body string) error
	DeleteItem(path string) error
}

type filesystemStore struct {
	repo  *repo.Repo
	cache *cache.Cache
}

func newFilesystemStore(r *repo.Repo, c *cache.Cache) ItemStore {
	return filesystemStore{repo: r, cache: c}
}

func (s filesystemStore) All() ([]scanner.Item, error) {
	return s.cache.All()
}

func (s filesystemStore) GetByID(id string) (*scanner.Item, error) {
	return s.cache.GetByID(id)
}

func (s filesystemStore) ReadRaw(path string) (string, error) {
	return s.repo.ReadRaw(path)
}

func (s filesystemStore) WriteRaw(path, content string) error {
	if err := s.repo.WriteRaw(path, content); err != nil {
		return err
	}
	return s.cache.NotifyWrite(path)
}

func (s filesystemStore) CreateItem(path string, fm map[string]interface{}, body string) error {
	return s.repo.CreateItem(path, fm, body)
}

func (s filesystemStore) UpdateItem(path string, fm map[string]interface{}, body string) error {
	return s.repo.UpdateItem(path, fm, body)
}

func (s filesystemStore) DeleteItem(path string) error {
	return s.repo.DeleteItem(path)
}
