package version

import "fmt"

var (
	Version   = "dev"
	BuildTime = "unknown"
)

func String() string {
	return fmt.Sprintf("exokephalos %s (built %s)", Version, BuildTime)
}
