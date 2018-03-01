#include <forward_list>
#include "gui.hpp"
#include "panel.hpp"
#include "../modules/modules.hpp"
#include <string>

#ifndef UI_HPP
#define UI_HPP

class UI {
    public:
        UI();
        void event_loop();
        bool register_ui_module(const UI_module &module);
        void start_gui();
    private:
        ui_module_list_t modules{};
        BS5panel    *panel;
        BS5gui      *gui;
        bool running = true;
};
#endif
