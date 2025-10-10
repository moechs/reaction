set -e

if test "$(realpath "$PWD")" != "$(realpath "$(dirname "$0")/..")"
then
	echo "You must be in reaction root directory"
	exit 1
fi

if test ! -f "$1"
then
	# shellcheck disable=SC2016
	echo '$1 must be a configuration file (most probably in ./bench)'
	exit 1
fi

rm -f reaction.db 
cargo build --release --bins
sudo systemd-run --wait \
	-p User="$(id -nu)" \
	-p MemoryAccounting=yes \
	-p IOAccounting=yes \
	-p WorkingDirectory="$(pwd)" \
	-p Environment=PATH=/run/current-system/sw/bin/ \
	sh -c "for i in 1 2; do ./target/release/reaction start -c '$1' -l ERROR -s ./reaction.sock; done"
