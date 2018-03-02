#include <boost/log/trivial.hpp>
#include "ui.hpp"
#include "gui.hpp"
#include <map>
#include <string>

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

bool BS5gui::sdl_init()
{
    //Initialization flag
    bool success = true;

    //initialize TTF
    if(TTF_Init()==-1) {
        printf("TTF_Init: %s\n", TTF_GetError());
        exit(2);
    }

    //Initialize SDL
    if( SDL_Init( SDL_INIT_VIDEO ) < 0 )
    {
        printf( "SDL could not initialize! SDL_Error: %s\n", SDL_GetError() );
        success = false;
    }
    else
    {
        //Create window
        this->window = SDL_CreateWindow( "neomedia", SDL_WINDOWPOS_UNDEFINED, SDL_WINDOWPOS_UNDEFINED, SCREEN_WIDTH, SCREEN_HEIGHT, SDL_WINDOW_SHOWN);
        if( this->window == NULL )
        {
            printf( "Window could not be created! SDL_Error: %s\n", SDL_GetError() );
            success = false;
        }
        else
        {
            //Get window surface
            this->window_surface = SDL_GetWindowSurface( this->window );
        }
    }

    return success;
}

BS5gui::BS5gui(ui_module_list_t & ui_module_list) {
    sdl_init();
    this->module_wheel = new ModuleWheel(this->window, ui_module_list);
    render();
}

void BS5gui::handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state) {
	if (damage.wheel_3) {
		this->module_wheel->handle_panel_input(damage, state);
	}
	this->render();
}

void BS5gui::render() {
    //BOOST_LOG_TRIVIAL(debug) << "Rendering frame";
	if (this->panel != nullptr) {
		this->module_wheel->bg_color = {
			(uint8_t)(this->panel->input.bs5_state.posWheel1 % 255),
			(uint8_t)(this->panel->input.bs5_state.posWheel2 % 255),
			(uint8_t)(this->panel->input.bs5_state.posWheel3),
					(uint8_t)255 };
	} else {
		this->module_wheel->bg_color = {0xff, 0xf0, 0x0f, 0xff};
	}
    this->module_wheel->render();
    for(auto layer: this->layer_list) {
//        SDL_BlitSurface(*layer, 0, this->window_surface, 0);
    }
    //SDL_UpdateWindowSurface(this->window);
}