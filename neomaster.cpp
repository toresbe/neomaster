#include <boost/thread.hpp>
#include "modules/modules.hpp"
#include "ui/ui.hpp"
#include "SDL.h"

int main(int argc, char *argv[]) {
    UI ui;
    ModuleManager manager;
    for( auto module: manager.module_list ) 
        ui.register_module(module);
    ui.start_gui();
    boost::thread ui_thread(boost::bind(&UI::event_loop, &ui));

    ui_thread.join();
    return 0;
}
