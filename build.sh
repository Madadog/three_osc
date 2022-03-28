# run port generator, copy generated .ttl
cd ./crates/port_generator
cargo run
cd ../..

cp -v ./crates/port_generator/three_osc.ttl ./three_osc.lv2/

# build plugin
cargo build --release
# copy to user lv2 folder
./copy_lv2.sh