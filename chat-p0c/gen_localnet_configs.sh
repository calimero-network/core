# Check if an argument was provided
if [ $# -eq 0 ]; then
    echo "Please provide the number of local nodes argument."
    exit 1
fi


# Get the first command line argument
N=$1

# Iterate in a loop N times
for (( i=1; i<=N; i++ ))
do
  rm -rf ~/.calimero/node$i
  mkdir -p ~/.calimero/node$i
  cargo run -- --home ~/.calimero/node$i init --port 233$i
done
