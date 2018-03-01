#include <libusb-1.0/libusb.h>
#include <forward_list>
#include <string>
#include <boost/exception/all.hpp>
#include <iostream>
#ifndef PANEL_HPP

#define PANEL_HPP

#define PANEL_BACKLIGHT = 0x4000

typedef boost::error_info<struct tag_bs5_panel_exception_info, std::string> bs5_panel_exception_info;
struct bs5_panel_exception: virtual boost::exception, virtual std::exception { };

class BS5Input {
    public:
        typedef enum event_t {
            wheel1,
            wheel2,
            wheel3,
            button_press,
            button_release,
            all
        } event_t;
        typedef struct bs5_state {
            int posWheel1;
            int posWheel2;
            int posWheel3;
            bool button_left;
            bool button_right;
            bool button_ok;
        } bs5_state_t;
        bs5_state_t bs5_state;
        typedef void (* input_callback_function_t)(event_t event, bs5_state_t &bs5_state); 
        typedef struct callback_entry {event_t event; input_callback_function_t *f;} callback_entry;
        typedef struct bs5_message {
            int8_t wheel_1;             // Outermost wheel
            int8_t wheel_2;
            uint8_t wheel_3;
            uint8_t buttons;            // 0x20 = <-, 0x10 = ->, 0x40 = OK
            uint8_t b5;
            uint8_t b6;
        } bs5_message;
        void fire_event(const event_t event);
        void new_data(const bs5_message &msg);
        void add_callback(event_t event_filter, input_callback_function_t *callback);
    private:
        std::forward_list<callback_entry> callback_list;
};

class BS5panel {
    public:
        void set_backlight(bool is_powered);
        BS5panel();
        void read_status();
        void click();
        BS5Input input{};
    private:
        typedef uint8_t status_flag_t[2];
        status_flag_t status_flags = {0, 0};
        void send_status(status_flag_t status_flags);
        //void print_status(const BS5panel::bs5_message &msg);
        libusb_device_handle *device_handle;
        bool open_device();
};
#endif
