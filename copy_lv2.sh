# Script that copies the plugin into a location where hosts (i.e. Ardour or Carla) can find it.

# First, copy the LV2 binary into the three_osc.lv2 directory
cp -v ./target/release/libthree_osc.so ./three_osc.lv2/

# Second, copy the LV2 plugin (whole directory) into the user's home .lv2 directory
cp -v -r ./three_osc.lv2 ~/.lv2/
