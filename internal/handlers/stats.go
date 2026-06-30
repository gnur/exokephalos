package handlers

import (
	"fmt"
	"net/http"
	"sort"
	"time"

	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/scanner"
)

// StatsBuilder computes stats data from a list of items.
type StatsBuilder func(items []scanner.Item) map[string]interface{}

// statsBuilders maps stats_template names to their builder functions.
var statsBuilders = map[string]StatsBuilder{
	"books/stats": buildBooksStats,
}

// ViewStats handles GET /views/{viewId}/stats.
func (h *Handlers) ViewStats(w http.ResponseWriter, r *http.Request) {
	viewID := r.PathValue("viewId")
	viewCfg, ok := h.Cfg.Views[viewID]
	if !ok {
		http.NotFound(w, r)
		return
	}

	if viewCfg.StatsTemplate == "" {
		http.NotFound(w, r)
		return
	}

	builder, ok := statsBuilders[viewCfg.StatsTemplate]
	if !ok {
		http.Error(w, fmt.Sprintf("unknown stats_template: %q", viewCfg.StatsTemplate), 500)
		return
	}

	data := newData(r)
	parseStart := time.Now()

	items, err := h.scanAndFilter(viewID)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	// Build stats data and merge into template data
	statsData := builder(items)
	for k, v := range statsData {
		data[k] = v
	}

	data["_parseTime"] = time.Since(parseStart)
	data["View"] = viewCfg
	data["ViewID"] = viewID

	h.render(w, r, viewCfg.StatsTemplate+".html", data)
}

// --- Books Stats Builder ---

type yearStat struct {
	Year  string
	Count int
	Pages int
}

func buildBooksStats(items []scanner.Item) map[string]interface{} {
	// Categorize by tags
	var read, reading, toRead []scanner.Item
	for _, item := range items {
		tags := item.GetTags()
		for _, t := range tags {
			switch t {
			case "read":
				read = append(read, item)
			case "reading":
				reading = append(reading, item)
			case "to-read":
				toRead = append(toRead, item)
			}
		}
	}

	// Year stats from "read" books
	yearStats := map[string]*yearStat{}
	for _, b := range read {
		finished := markdown.FMString(b.Frontmatter, "finished")
		if finished != "" && len(finished) >= 4 {
			y := finished[:4]
			if _, ok := yearStats[y]; !ok {
				yearStats[y] = &yearStat{Year: y}
			}
			yearStats[y].Count++
			yearStats[y].Pages += markdown.FMInt(b.Frontmatter, "pages")
		}
	}

	var years []yearStat
	for _, s := range yearStats {
		years = append(years, *s)
	}
	sort.Slice(years, func(i, j int) bool { return years[i].Year > years[j].Year })

	// Time to 1000
	currentYear := fmt.Sprintf("%d", time.Now().Year())
	lastYearStr := fmt.Sprintf("%d", time.Now().Year()-1)
	booksLastYear := 0
	if s, ok := yearStats[lastYearStr]; ok {
		booksLastYear = s.Count
	}
	if booksLastYear == 0 {
		if s, ok := yearStats[currentYear]; ok {
			booksLastYear = s.Count
		}
	}

	remaining := 1000 - len(read)
	yearsTo1000 := "N/A"
	if booksLastYear > 0 && remaining > 0 {
		y := float64(remaining) / float64(booksLastYear)
		yearsTo1000 = fmt.Sprintf("%.1f years (at %d books/year)", y, booksLastYear)
	} else if remaining <= 0 {
		yearsTo1000 = "Already reached!"
	}

	return map[string]interface{}{
		"TotalRead":      len(read),
		"Reading":        len(reading),
		"ToRead":         len(toRead),
		"YearCounts":     years,
		"YearsTo1000":    yearsTo1000,
		"BooksRemaining": remaining,
	}
}

