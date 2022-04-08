# script that automates .lv2 boilerplate (makes the .ttl file), generates a port list,
# compiles the shared library, and copies the library and .ttl files to the user's home .lv2 folder

# run port generator, copy generated .ttl
cd ./crates/port_generator
cargo run
cd ../..

cp -v ./crates/port_generator/three_osc.ttl ./three_osc.lv2/

# build plugin
cargo build --release
# copy to user lv2 folder
./copy_lv2.sh