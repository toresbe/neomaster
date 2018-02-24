#include "../modules/modules.hpp"
#include "SDL2/SDL.h"
#ifndef GUI_HPP

#define GUI_HPP
class ModuleWheel {
    public:
        void module_wheel_moved();
        ModuleWheel(ui_module_list_t module_list);
        SDL_Surface *surface;
    private:
};

class BS5gui {
    public:
        BS5gui(ui_module_list_t & ui_module_list);
        void render();
    private:
        ModuleWheel* module_wheel;
        bool sdl_init();
        SDL_Window* window;
        SDL_Surface* window_surface;
        std::forward_list<SDL_Surface **> layer_list;
};
#endif
