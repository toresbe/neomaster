#include "../modules/modules.hpp"
#include "panel.hpp"

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

#define SCREEN_WIDTH 1024
#define SCREEN_HEIGHT 768
#ifndef GUI_HPP

#define GUI_HPP
class ModuleWheel {
    public:
        void module_wheel_moved();
		void render_background();
        ModuleWheel(SDL_Window * window, ui_module_list_t module_list);
        void render();
		void handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state);
        SDL_Color fg_color={255, 255, 255, 255};
        SDL_Color bg_color={255, 255, 255, 255};
    private:
		BS5input::bs5_state_t input_state;
		SDL_Renderer * renderer;
		void render_text(const std::string & text, SDL_Rect rect);
        void render_modules();
		SDL_Rect get_rekt(int num_modules, int module_index);
		void render_beam();
        ui_module_list_t module_list;
        TTF_Font *font;
        int font_line_height;
};

class BS5gui {
    public:
        BS5gui(ui_module_list_t & ui_module_list);
		void handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state);
        void render();
        BS5panel *panel = nullptr;
    private:
        ModuleWheel* module_wheel;
        bool sdl_init();
        SDL_Window* window;
        SDL_Surface* window_surface;
        std::forward_list<SDL_Surface **> layer_list;
};
#endif
