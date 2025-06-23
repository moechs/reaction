set -e

rm -f reaction.db 
cargo build --release 
sudo systemd-run --wait \
	-p User="$(id -nu)" \
	-p MemoryAccounting=yes \
	-p IOAccounting=yes \
	-p WorkingDirectory="$(pwd)" \
	-p Environment=PATH=/run/current-system/sw/bin/ \
	sh -c "for i in 1 2; do ./target/release/reaction start -c ./bench/heavy-load.yml -l ERROR -s ./reaction.sock; done"
