#include <boost/log/trivial.hpp>
#include "ui.hpp"
#include "gui.hpp"
#include "SDL2/SDL.h"

#define SCREEN_WIDTH 1024
#define SCREEN_HEIGHT 768

void ModuleWheel::module_wheel_moved() { };

ModuleWheel::ModuleWheel(ui_module_list_t module_list) {
    this->surface = SDL_CreateRGBSurface(0, SCREEN_WIDTH, SCREEN_HEIGHT, 32, 0,0,0,0);
    SDL_FillRect(this->surface, 0, SDL_MapRGB(this->surface->format, 255, 0, 0));
}

    bool render_frame() {
    };

    bool BS5gui::sdl_init()
    {
        //Initialization flag
        bool success = true;

        //Initialize SDL
        if( SDL_Init( SDL_INIT_VIDEO ) < 0 )
        {
            printf( "SDL could not initialize! SDL_Error: %s\n", SDL_GetError() );
            success = false;
        }
        else
        {
            //Create window
            this->window = SDL_CreateWindow( "neomedia", SDL_WINDOWPOS_UNDEFINED, SDL_WINDOWPOS_UNDEFINED, SCREEN_WIDTH, SCREEN_HEIGHT, SDL_WINDOW_SHOWN );
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
    this->module_wheel = new ModuleWheel(ui_module_list);
    this->layer_list.push_front(&this->module_wheel->surface);
}

void BS5gui::render() {
    BOOST_LOG_TRIVIAL(debug) << "Rendering frame";
    SDL_FillRect(this->module_wheel->surface, 0, SDL_MapRGB(this->module_wheel->surface->format, 255, 0, 0));
    SDL_BlitSurface(this->module_wheel->surface, 0, this->window_surface, 0);
    SDL_UpdateWindowSurface(this->window);
}
