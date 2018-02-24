#include <libusb-1.0/libusb.h>
#include <string>
#include <boost/exception/all.hpp>
#include <iostream>
#ifndef PANEL_HPP

#define PANEL_HPP

#define PANEL_BACKLIGHT = 0x4000

typedef boost::error_info<struct tag_bs5_panel_exception_info, std::string> bs5_panel_exception_info;
struct bs5_panel_exception: virtual boost::exception, virtual std::exception { };


class BS5panel {
    public:
        void set_backlight(bool is_powered);
        int posWheel1;
        int posWheel2;
        BS5panel();
        typedef struct bs5_message {
            int8_t wheel_1;             // Outermost wheel
            int8_t wheel_2;
            uint8_t b3;
            uint8_t b4;
            uint8_t b5;
            uint8_t b6;
        } bs5_message;
        bs5_message read_status();
    private:
        uint8_t status_flags[2] = {0, 0};
        void send_status();
        void print_status(const BS5panel::bs5_message &msg);
        libusb_device_handle *device_handle;
        bool open_device();
};
#endif
