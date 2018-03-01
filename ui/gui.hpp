#include "../modules/modules.hpp"
#include "panel.hpp"

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

#ifndef GUI_HPP

#define GUI_HPP
class ModuleWheel {
    public:
        void module_wheel_moved();
        ModuleWheel(ui_module_list_t module_list);
        SDL_Surface *render();
        SDL_Surface *surface;
        SDL_Color fg_color={255, 255, 255, 255};
        SDL_Color bg_color={255, 255, 255, 255};
    private:
        void render_modules();
        void render_text(std::string text, SDL_Surface *target_surface, int height);
        ui_module_list_t module_list;
        TTF_Font *font;
        int font_line_height;
};

class BS5gui {
    public:
        BS5gui(ui_module_list_t & ui_module_list);
        void render();
        BS5panel *panel = nullptr;
    private:
        void input_callback(BS5input::event_t event, BS5input::bs5_state_t state);
        ModuleWheel* module_wheel;
        bool sdl_init();
        SDL_Window* window;
        SDL_Surface* window_surface;
        std::forward_list<SDL_Surface **> layer_list;
};
#endif
