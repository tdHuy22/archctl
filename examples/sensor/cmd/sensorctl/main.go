package main

import (
	"fmt"

	"github.com/acme/sensor/internal/stream"
)

func main() {
	fmt.Println(stream.Open("eth0"))
}
