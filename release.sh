#!/usr/bin/env bash

set -exu

git push --tags

TAG="$(git tag --sort=v:refname | tail -n1)"

echo -n "We will create a release for tag $TAG. Do you want to continue? (y/n) "

docker run -it --rm \
	-e HOME=/tmp/ \
	-v "$(pwd)":/tmp/code -w /tmp/code \
	-u "$(id -u)" \
	rust \
	make reaction_"${TAG:1}"-1_amd64.deb reaction ip46tables nft46

make "signatures_${TAG:1}"

rsync -avz -e 'ssh -J pica01' \
	./ip46tables ./ip46tables.minisig \
	./nft46 ./nft46.minisig \
	./target/release/reaction ./reaction.minisig \
	./reaction_"${TAG:1}"-1_amd64.deb ./reaction_"${TAG:1}"-1_amd64.deb.minisig \
	./target/release/reaction.bash ./target/release/reaction.fish ./target/release/_reaction \
	./target/release/reaction*.1
	akesi:/var/www/static/reaction/releases/"$TAG"

TOKEN="$(rbw get framagit.org token)"

DESCRIPTION="
## Changes

## Download

\`\`\`bash
wget https://static.ppom.me/reaction/releases/""$TAG""/nft46 \\
  https://static.ppom.me/reaction/releases/""$TAG""/reaction \\
  https://static.ppom.me/reaction/releases/""$TAG""/ip46tables \\
  https://static.ppom.me/reaction/releases/""$TAG""/nft46.minisig \\
  https://static.ppom.me/reaction/releases/""$TAG""/reaction.minisig \\
  https://static.ppom.me/reaction/releases/""$TAG""/ip46tables.minisig
for i in nft46 ip46tables reaction
do
    minisign -VP RWSpLTPfbvllNqRrXUgZzM7mFjLUA7PQioAItz80ag8uU4A2wtoT2DzX -m $i && rm $i.minisig
done
\`\`\`
		
**Debian Installation**

\`\`\`bash
wget https://static.ppom.me/reaction/releases/""$TAG""/reaction_""${TAG:1}""-1_amd64.deb \\
https://static.ppom.me/reaction/releases/""$TAG""/reaction_""${TAG:1}""-1_amd64.deb.minisig
minisign -VP RWSpLTPfbvllNqRrXUgZzM7mFjLUA7PQioAItz80ag8uU4A2wtoT2DzX -m reaction_""${TAG:1}""-1_amd64.deb &&
	rm reaction_""${TAG:1}""-1_amd64.deb.minisig &&
	apt install ./reaction_""${TAG:1}""-1_amd64.deb
\`\`\`
"

DATA='{
"tag_name":"'"$TAG"'",
"description": "'"$DESCRIPTION"'",
"assets":{"links":[
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/nft46", "name": "nft46 (x86-64)", "link_type": "package"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/reaction", "name": "reaction (x86-64)", "link_type": "package"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/ip46tables", "name": "ip46tables (x86-64)", "link_type": "package"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/reaction_'"${TAG:1}"'-1_amd64.deb", "name": "reaction_'"${TAG:1}"'-1_amd64.deb (x86-64)", "link_type": "package"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/nft46.minisig", "name": "nft46.minisig", "link_type": "other"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/reaction.minisig", "name": "reaction.minisig", "link_type": "other"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/ip46tables.minisig", "name": "ip46tables.minisig", "link_type": "other"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/reaction_'"${TAG:1}"'-1_amd64.deb.minisig", "name": "reaction_'"${TAG:1}"'-1_amd64.deb.minisig", "link_type": "other"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/reaction.bash", "name": "bash shell completion", "link_type": "other"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/reaction.fish", "name": "fish shell completion", "link_type": "other"},
{"url": "https://static.ppom.me/reaction/releases/'"$TAG"'/_reaction", "name": "zsh shell completion", "link_type": "other"}
]}}'

curl \
	--fail-with-body \
	--location \
	-X POST \
	-H 'Content-Type: application/json' \
	-H "PRIVATE-TOKEN: $TOKEN" \
	'https://framagit.org/api/v4/projects/90566/releases' \
	--data "$DATA"

make clean
