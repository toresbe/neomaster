#include <libusb-1.0/libusb.h>
#include <forward_list>
#include <string>
#include <boost/exception/all.hpp>
#include <iostream>
#ifndef PANEL_HPP

#define PANEL_HPP

#define PANEL_BACKLIGHT = 0x4000

typedef struct bs5_message_t {
	int8_t wheel_1;             // Outermost wheel
	int8_t wheel_2;
	uint8_t wheel_3;
	uint8_t buttons;            // 0x20 = <-, 0x10 = ->, 0x40 = OK
	uint8_t b5;
	uint8_t b6;
} bs5_message_t;



typedef boost::error_info<struct tag_bs5_panel_exception_info, std::string> bs5_panel_exception_info;
struct bs5_panel_exception: virtual boost::exception, virtual std::exception { };

class BS5input {
    public:
		typedef struct bs5_damage_t {
			bool wheel_1;
			bool wheel_2;
			bool wheel_3;
			bool buttons;
		} bs5_damage_t;
        typedef struct bs5_state {
            int posWheel1;
            int posWheel2;
            unsigned int posWheel3;
            bool button_left;
            bool button_right;
            bool button_ok;
        } bs5_state_t;
        bs5_state_t bs5_state;
        void parse_message(const bs5_message_t &msg);
    private:

};

class BS5panel {
    public:
        virtual void set_backlight(bool is_powered) = 0;
		virtual	BS5input::bs5_damage_t read_status() = 0;
		virtual void click() = 0;
		BS5input input;
};

class USBBS5panel : public BS5panel {
public:
	void set_backlight(bool is_powered);
	USBBS5panel();
	BS5input::bs5_damage_t read_status();
	void click();
private:
	typedef uint8_t status_flag_t[2];
	status_flag_t status_flags = { 0, 0 };
	void send_status(status_flag_t status_flags);
	//void print_status(const USBBS5panel::bs5_message_t &msg);
	libusb_device_handle *device_handle;
	bool open_device();
};

class FakeBS5panel : public BS5panel {
public:
	void set_backlight(bool is_powered);
	void click();
	BS5input::bs5_damage_t read_status();
private:
	int phase = 4;
	void send_status();
};
#endif
