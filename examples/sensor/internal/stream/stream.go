package stream

import (
	"net"

	// Vi phạm: tầng internal import ngược lên cmd.
	_ "github.com/acme/sensor/cmd/sensorctl"
)

func Open(iface string) net.Addr {
	return nil
}
