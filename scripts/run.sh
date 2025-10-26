#!/bin/bash

if [ $# -eq 0 ]; then
    echo "Usage: $0 <qdisc-config>"
    echo "Example: $0 'pfifo limit 1000'"
    exit 1
fi

sudo ./target/debug/phantomlink setup --qdisc-client "$1"

sudo ./target/debug/phantomlink start examples/input.csv &
START_PID=$!

sleep 1
# Run server and capture output in background
sudo ./target/debug/phantomlink exec server iperf3 -s --port 5000 > server_output.txt 2>&1 &
SERVER_PID=$!

sleep 1 
# Run client and capture its output
sudo ./target/debug/phantomlink exec client iperf3 -c 192.168.66.2 --port 5000 > client_output.txt 2>&1 &
CLIENT_PID=$!

wait $CLIENT_PID
kill $SERVER_PID
kill $START_PID


# sudo ./target/debug/phantomlink exec <client/server> ifconfig
sudo ./target/debug/phantomlink teardown

sleep 1

# Reset terminal state bc iperf3 messes with it
stty sane

echo "Client output:"
cat client_output.txt
echo "Server output:"
cat server_output.txt

# Cleanup temp files
rm -f server_output.txt client_output.txt
