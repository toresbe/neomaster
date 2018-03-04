#include <forward_list>
#include "gui.hpp"
#include "panel.hpp"
#include "../modules/modules.hpp"
#include <string>

#ifndef UI_HPP
#define UI_HPP

class UI {
    public:
        void event_loop();
        bool register_module(NeomasterModule *module);
        void start_gui();
    private:
        module_list_t modules{};
        BS5panel    *panel;
        NeomasterUI      *gui;
        bool running = true;
};
#endif
