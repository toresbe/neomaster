#include <libusb-1.0/libusb.h>
#include <iostream>
#include "buttons.hpp"
#include "boost/format.hpp"

Buttons::Buttons(void) {
    std::cout << "Object is being created" << std::endl;
    libusb_init(NULL);
    Buttons::find_device();
}

bool is_interesting(libusb_device *device) {
    struct libusb_device_descriptor desc;
    libusb_get_device_descriptor(device, &desc);
    std::cout << "candidate:" << std::endl;
    std::cout << boost::format("0x%04X\t0x%04X\n") % desc.idVendor % desc.idProduct;
    return false;
}

libusb_device *find_device() {
    return NULL;
};

bool Buttons::find_device() {
    // discover devices
    libusb_device **list;
    libusb_device *found = NULL;
    ssize_t cnt = libusb_get_device_list(NULL, &list);
    ssize_t i = 0;
    int err = 0;
    if (cnt < 0)
        return false;
    for (i = 0; i < cnt; i++) {
        libusb_device *device = list[i];
        if (is_interesting(device)) {
            found = device;
            break;
        }
    }
    if (found) {
        libusb_device_handle *handle;
        err = libusb_open(found, &handle);
        if (err)
        return false;
}
    libusb_free_device_list(list, 1);
    return false;
}

//bool Buttons::read_data(buffer) {
//    this->last_state
//}
